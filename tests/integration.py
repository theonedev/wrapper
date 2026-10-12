#!/usr/bin/env python3
"""Run a real JVM in a temporary distribution; never register a machine service."""
import os
from pathlib import Path
import platform
import shutil
import signal
import socket
import subprocess
import tempfile
import time
import unittest

PROJECT = Path(__file__).resolve().parents[1]
BINARY_NAME = {
    ("Darwin", "arm64"): "wrapper-macosx-arm-64",
    ("Linux", "x86_64"): "wrapper-linux-x86-64",
    ("Linux", "aarch64"): "wrapper-linux-arm-64",
    ("Windows", "AMD64"): "wrapper-windows-x86-64.exe",
    ("Windows", "ARM64"): "wrapper-windows-arm-64.exe",
}[platform.system(), platform.machine()]


class WrapperIntegration(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.classes = PROJECT / "build" / "fixture"
        cls.classes.mkdir(parents=True, exist_ok=True)
        subprocess.run(["javac", "--release", "17", "-cp", str(PROJECT / "dist/boot/wrapper.jar"),
                        "-d", str(cls.classes), str(PROJECT / "tests/Fixture.java")], check=True)

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="OneDev wrapper space ")
        self.root = Path(self.temp.name)
        for directory in ("boot", "bin", "conf", "logs"):
            (self.root / directory).mkdir()
        self.binary = self.root / "boot" / BINARY_NAME
        shutil.copy2(PROJECT / "dist/boot" / BINARY_NAME, self.binary)
        shutil.copy2(PROJECT / "dist/boot/wrapper.jar", self.root / "boot/wrapper.jar")
        shutil.copy2(self.classes / "Fixture.class", self.root / "boot/Fixture.class")
        self.config = self.root / "conf/wrapper.conf"
        self.source = f'''#encoding=UTF-8
wrapper.name=fixture
wrapper.java.command=java
wrapper.java.classpath.1=.
wrapper.java.classpath.2=*.jar
wrapper.app.parameter.1=Fixture
wrapper.java.additional.100=-Dfixture.root={self.root}
wrapper.java.additional.2=-Dfixture.value=wrong-order
wrapper.java.additional.10=-Dfixture.value="before with spaces"
wrapper.shutdown.timeout=3
wrapper.startup.timeout=5
wrapper.restart.delay=0
wrapper.max_failed_invocations=2
'''
        self.config.write_text(self.source)
        self.processes = []

    def tearDown(self):
        try:
            self.command("stop", check=False, timeout=10)
        finally:
            for process in self.processes:
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait(timeout=5)
                if process.stdout:
                    process.stdout.close()
            self.temp.cleanup()

    def command(self, command, *args, check=True, **kwargs):
        return subprocess.run([str(self.binary), command, str(self.config), *args],
                              text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                              check=check, **kwargs)

    def events(self):
        path = self.root / "events"
        return path.read_text().splitlines() if path.exists() else []

    def wait_for(self, predicate, timeout=10):
        until = time.monotonic() + timeout
        while time.monotonic() < until:
            if predicate():
                return
            time.sleep(0.05)
        self.fail(f"Timed out. Events: {self.events()}. Log: {(self.root / 'logs/console.log').read_text() if (self.root / 'logs/console.log').exists() else ''}")

    def console(self, *args):
        process = subprocess.Popen([str(self.binary), "console", str(self.config), *args],
                                   stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        self.processes.append(process)
        return process

    def test_arguments_includes_defaults_and_exit_status(self):
        (self.root / "conf/extra.conf").write_text("set.default.fixture_test_value=hello world\nwrapper.java.additional.11=-Dfixture.value=%fixture_test_value%\n")
        self.config.write_text(self.source + "#include.required ../conf/extra.conf\n")
        result = self.command("console", "wrapper.java.additional.20=-Dfixture.mode=once", "--", "two words", 'x;$(touch nope)', '汉字', timeout=10)
        self.assertIn("property=hello world", result.stdout)
        for arg in ("two words", 'x;$(touch nope)', '汉字'):
            self.assertIn("arg=" + arg, result.stdout)
        self.assertFalse((self.root / "boot/nope").exists())
        result = self.command("console", "wrapper.java.additional.20=-Dfixture.mode=exit", "--", "one-shot", check=False, timeout=10)
        self.assertEqual(result.returncode, 7, result.stdout)

    @unittest.skipIf(os.name == "nt", "Windows start uses SCM; tested separately with service acceptance script")
    def test_detached_start_duplicate_auth_restart_reload_and_stop(self):
        self.command("start", timeout=10)
        self.wait_for(lambda: len(self.events()) >= 1)
        wrapper_pid = (self.root / "bin/fixture.pid").read_text()
        result = self.command("console", check=False, timeout=10)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("already running", result.stdout)
        control = (self.root / "bin/fixture.control").read_text().splitlines()
        with socket.create_connection(("127.0.0.1", int(control[0]))) as connection:
            connection.sendall(b"wrong-token stop\n")
            self.assertEqual(connection.recv(32), b"")
        # An idle connection must not block another client's status/stop requests.
        with socket.create_connection(("127.0.0.1", int(control[0]))) as connection:
            self.assertIn("RUNNING", self.command("status", timeout=2).stdout)
            time.sleep(0.5)
            connection.sendall(f"{control[1]} status\n".encode())
            self.assertEqual(connection.recv(32), b"RUNNING\n")
        self.assertIn("RUNNING", self.command("status").stdout)
        self.config.write_text(self.source.replace("before with spaces", "after reload"))
        (self.root / "restart").touch()
        self.wait_for(lambda: any(line.endswith("after reload") for line in self.events()))
        self.assertEqual((self.root / "bin/fixture.pid").read_text(), wrapper_pid)
        self.assertEqual(len([e for e in self.events() if e.startswith("start ")]), 2)
        self.command("stop", timeout=10)
        self.assertEqual(len([e for e in self.events() if e.startswith("stop ")]), 2)
        self.assertEqual(self.command("status", check=False).returncode, 3)
        self.command("start", timeout=10)
        self.command("stop", timeout=10)

    @unittest.skipIf(os.name == "nt", "POSIX signals")
    def test_signal_and_parent_death_run_shutdown_hooks(self):
        for sig in (signal.SIGTERM, signal.SIGKILL):
            before = len(self.events())
            process = self.console()
            self.wait_for(lambda: len(self.events()) > before)
            process.send_signal(sig)
            process.wait(timeout=10)
            self.wait_for(lambda: len(self.events()) >= before + 2)

    def test_application_stop(self):
        process = self.console()
        self.wait_for(lambda: len(self.events()) >= 1)
        (self.root / "stop").touch()
        self.assertEqual(process.wait(timeout=10), 0)
        self.assertTrue(self.events()[-1].startswith("stop "))

    def test_crash_restarts_are_bounded(self):
        result = self.command("console", "wrapper.java.additional.20=-Dfixture.mode=crash", check=False, timeout=15)
        self.assertEqual(result.returncode, 9, result.stdout)
        self.assertEqual(len([e for e in self.events() if e.startswith("start ")]), 2)

    def test_shutdown_timeout(self):
        process = self.console("wrapper.java.additional.20=-Dfixture.mode=hang", "wrapper.shutdown.timeout=1")
        self.wait_for(lambda: len(self.events()) >= 1)
        self.command("stop", timeout=10)
        self.assertEqual(process.wait(timeout=10), 1)
        self.assertIn("Shutdown timeout expired", (self.root / "logs/console.log").read_text())

    @unittest.skipIf(os.name == "nt", "POSIX executable fixture")
    def test_startup_timeout(self):
        executable = self.root / "fake java"
        executable.write_text('#!/bin/sh\nif [ "$1" = "-version" ]; then\n echo \'openjdk version "17.0.1"\' >&2\n exit 0\nfi\nsleep 60\n')
        executable.chmod(0o755)
        result = self.command("console", f"wrapper.java.command={executable}", "wrapper.startup.timeout=1", check=False, timeout=10)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout.count("JVM bridge startup timed out"), 2)

    def test_rotation(self):
        result = self.command("console", "wrapper.java.additional.20=-Dfixture.mode=logs", "wrapper.logfile.maxsize=2k", "wrapper.logfile.maxfiles=2", timeout=15)
        self.assertEqual(result.returncode, 0)
        self.assertTrue((self.root / "logs/console.log.2").exists())
        self.assertFalse((self.root / "logs/console.log.3").exists())

    @unittest.skipIf(os.name == "nt", "Bash launcher")
    def test_launcher_and_utility_arguments(self):
        self.config.write_text(self.source + "wrapper.java.additional.20=-Dfixture.mode=once\n")
        for name, arguments in (("server", ["console", "one two"]), ("agent", ["console", "one two"]), ("my-java-app", ["console", "one two"]), ("upgrade", ["one two"])):
            path = self.root / "bin" / (name + ".sh")
            launcher = (PROJECT / "resources/App.sh.in").read_text()
            if name == "upgrade":
                launcher = launcher.replace(" --script ", " --command ")
            path.write_text(launcher)
            path.chmod(0o755)
            result = subprocess.run([str(path), *arguments], cwd="/", capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("arg=one two", result.stdout)
            if name == "upgrade":
                self.assertIn("arg=upgrade", result.stdout)

    @unittest.skipIf(os.name == "nt", "Bash launcher")
    def test_generic_main_with_arguments_restarts_but_commands_do_not(self):
        self.config.write_text(self.source.replace("wrapper.app.parameter.1=Fixture", "wrapper.java.mainclass=Fixture")
                               + "wrapper.java.additional.20=-Dfixture.mode=crash\n")
        path = self.root / "bin/my-java-app.sh"
        source = (PROJECT / "resources/App.sh.in").read_text()
        path.write_text(source)
        path.chmod(0o755)
        result = subprocess.run([str(path), "console", "--port", "8080"], capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 9, result.stdout + result.stderr)
        self.assertEqual(len(self.events()), 2, self.events())
        path.write_text(source.replace(" --script ", " --command "))
        result = subprocess.run([str(path), "migrate"], capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 9, result.stdout + result.stderr)
        self.assertEqual(len(self.events()), 3, self.events())

    def test_include_cycle_fails(self):
        self.config.write_text(self.source + "#include.required ../conf/wrapper.conf\n")
        result = self.command("check", check=False, timeout=10)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Recursive", result.stdout)


if __name__ == "__main__":
    unittest.main(verbosity=2)
