#!/usr/bin/env python3
"""Exercise Linux PID 1 reaping in disposable Docker containers, without --init."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import unittest
import uuid

PROJECT = Path(__file__).resolve().parents[1]
IMAGE = os.environ.get("WRAPPER_TEST_IMAGE", "eclipse-temurin:17-jdk-jammy")


class Pid1Integration(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        architecture = subprocess.check_output(
            ["docker", "run", "--rm", "--network", "none", "--entrypoint", "uname", IMAGE, "-m"],
            text=True).strip()
        cls.binary = {"aarch64": "wrapper-linux-arm-64", "x86_64": "wrapper-linux-x86-64"}[architecture]

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="wrapper-pid1-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for name in (self.binary, "wrapper.jar"):
            shutil.copy2(PROJECT / "dist/boot" / name, self.root / name)
        subprocess.run(["javac", "--release", "17", "-cp", str(self.root / "wrapper.jar"),
                        "-d", str(self.root), str(PROJECT / "tests/Pid1Fixture.java")], check=True)
        (self.root / "wrapper.conf").write_text("""wrapper.java.command=java
wrapper.java.classpath.1=/fixture
wrapper.app.parameter.1=Pid1Fixture
wrapper.pidfile=/tmp/wrapper.pid
wrapper.logfile=/tmp/wrapper.log
wrapper.startup.timeout=5
wrapper.shutdown.timeout=5
wrapper.restart.delay=3
""")
        self.name = "wrapper-pid1-" + uuid.uuid4().hex
        self.addCleanup(lambda: subprocess.run(["docker", "rm", "-f", self.name],
                                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))

    def run_wrapper(self, mode):
        subprocess.run(["docker", "run", "-d", "--network", "none", "--name", self.name,
                        "--mount", f"type=bind,src={self.root},dst=/fixture,readonly",
                        "--entrypoint", "/fixture/" + self.binary, IMAGE,
                        "/fixture/wrapper.conf", "--", mode], check=True, capture_output=True)

    def logs(self):
        return subprocess.check_output(["docker", "logs", self.name], text=True, stderr=subprocess.STDOUT)

    def wait_for_log(self, text):
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            if text in self.logs():
                return
            time.sleep(0.05)
        self.fail(self.logs())

    def assert_exit(self, code):
        actual = subprocess.check_output(["docker", "wait", self.name], text=True, timeout=20).strip()
        self.assertEqual(int(actual), code, self.logs())
        self.assertIn("All five adopted children reaped", self.logs())

    def test_exit_codes(self):
        for mode, code in (("0", 0), ("7", 7), ("crash", 9)):
            with self.subTest(mode=mode):
                try:
                    self.run_wrapper(mode)
                    self.assert_exit(code)
                finally:
                    subprocess.run(["docker", "rm", "-f", self.name], check=True, capture_output=True)

    def test_java_probe(self):
        executable = self.root / "java"
        executable.write_text("""#!/bin/sh
if [ "$1" = -version ]; then
    orphan=$(sh -c 'setsid sleep 0.1 </dev/null >/dev/null 2>&1 & echo $!')
    attempt=0
    while [ -e "/proc/$orphan" ]; do
        attempt=$((attempt + 1))
        if [ "$attempt" -gt 100 ]; then
            echo 'Version probe orphan was not reaped' >&2
            exit 1
        fi
        sleep 0.025
    done
    echo 'Version probe orphan reaped' >&2
fi
exec java "$@"
""")
        executable.chmod(0o755)
        config = self.root / "wrapper.conf"
        config.write_text(config.read_text().replace("wrapper.java.command=java", "wrapper.java.command=/fixture/java"))
        self.run_wrapper("0")
        self.assert_exit(0)

    def test_restart(self):
        self.run_wrapper("restart")
        self.wait_for_log("Restarting JVM")
        # The orphan exits during the three-second restart delay. Check it was
        # reaped before the next Java version probe/JVM starts.
        subprocess.run(["docker", "exec", self.name, "sh", "-c",
                        'sleep 1.5; test ! -e "/proc/$(cat /tmp/orphans)"'], check=True, timeout=5)
        self.assert_exit(0)
        self.assertIn("Restart preserved; orphans reaped", self.logs())

    def test_sigterm(self):
        self.run_wrapper("signal")
        self.wait_for_log("Ready for signal")
        subprocess.run(["docker", "kill", "--signal=TERM", self.name], check=True, capture_output=True)
        self.assert_exit(0)
        self.assertIn("Shutdown hook ran", self.logs())


if __name__ == "__main__":
    unittest.main(verbosity=2)
