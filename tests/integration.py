#!/usr/bin/env python3
"""Run a real JVM in a temporary distribution; never register a machine service."""
import os
from pathlib import Path
import platform
import plistlib
import shlex
import shutil
import signal
import socket
import subprocess
import sys
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
        self.temp = tempfile.TemporaryDirectory(prefix="ServiceWrapper space ")
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

    def process_alive(self, pid):
        result = subprocess.run(["ps", "-p", str(pid), "-o", "stat="], capture_output=True, text=True)
        return result.returncode == 0 and not result.stdout.strip().startswith("Z")

    def hanging_java_probe(self):
        executable = self.root / "fake java"
        executable.write_text('#!/bin/sh\necho $$ > probe-pid\nsleep 60 &\necho $! > probe-child\nwait\n')
        executable.chmod(0o755)
        return f"wrapper.java.command={executable}"

    def assert_probe_stopped(self):
        for name in ("probe-pid", "probe-child"):
            pid = int((self.root / "boot" / name).read_text())
            self.wait_for(lambda: not self.process_alive(pid), timeout=3)

    @unittest.skipIf(os.name == "nt", "Detached Unix startup")
    def test_detached_start_validates_wait_before_spawning(self):
        for timeout in ("invalid", "-1"):
            with self.subTest(timeout=timeout):
                result = self.command("start", "wrapper.startup.wait.timeout=" + timeout,
                                      check=False, timeout=5)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("Invalid integer for wrapper.startup.wait.timeout", result.stdout)
                self.assertFalse((self.root / "logs/console.log").exists())
                self.assertEqual(self.command("status", check=False, timeout=3).returncode, 3)
                self.assertEqual(self.events(), [])

    @unittest.skipIf(os.name == "nt", "Detached Unix startup")
    def test_detached_start_unlimited_wait(self):
        self.command("start", "wrapper.startup.wait.timeout=0", timeout=10)
        self.assertIn("RUNNING", self.command("status", timeout=3).stdout)

    @unittest.skipIf(os.name == "nt", "Detached Unix startup")
    def test_detached_start_with_file_logging_disabled(self):
        # A regular file cannot be used as a directory, even when tests run as root.
        blocker = self.root / "log-blocker"
        blocker.write_text("unchanged")
        self.command("start", "wrapper.logfile.loglevel=NONE",
                     f"wrapper.logfile={blocker}/console.log", timeout=10)
        self.assertIn("RUNNING", self.command("status", timeout=3).stdout)
        self.assertEqual(blocker.read_text(), "unchanged")
        self.assertEqual(list((self.root / "logs").iterdir()), [])

    @unittest.skipIf(os.name == "nt", "Detached Unix startup")
    def test_detached_start_failure_with_file_logging_disabled(self):
        diagnostic = self.root / "disabled-logs/console.log"
        result = self.command("start", "wrapper.logfile.loglevel=NONE",
                              f"wrapper.logfile={diagnostic}",
                              f"wrapper.java.command={self.root}/missing-java",
                              check=False, timeout=10)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("failed to start", result.stdout)
        self.assertIn("run console for startup diagnostics", result.stdout)
        self.assertNotIn(str(diagnostic), result.stdout)
        self.assertFalse(diagnostic.parent.exists())

    @unittest.skipIf(os.name == "nt", "Unix helpers escaping the probe process group")
    def test_java_probe_with_detached_helper(self):
        executable = self.root / "fake java"
        script = self.root / "probe.py"
        executable.write_text("#!/bin/sh\nexec " + shlex.quote(sys.executable) + " "
                              + shlex.quote(str(script)) + "\n")
        executable.chmod(0o755)
        pidfile = self.root / "boot/escaped-pid"
        for mode in ("running", "exited", "stop"):
            with self.subTest(mode=mode):
                pidfile.unlink(missing_ok=True)
                script.write_text("import subprocess,time\nfrom pathlib import Path\n"
                                  "child = subprocess.Popen(['sleep', '60'], start_new_session=True)\n"
                                  "Path('escaped-pid').write_text(str(child.pid))\n"
                                  "print('openjdk version \\\"17.0.1\\\"', flush=True)\n"
                                  + ("time.sleep(60)\n" if mode == "running" else ""))
                process = subprocess.Popen(
                    [str(self.binary), "console" if mode == "stop" else "check", str(self.config),
                     "wrapper.java.command=" + str(executable),
                     "wrapper.startup.timeout=" + ("0" if mode == "stop" else "1")],
                    stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
                self.processes.append(process)
                try:
                    self.wait_for(lambda: pidfile.exists())
                    if mode == "stop":
                        self.command("stop", timeout=3)
                    output, _ = process.communicate(timeout=3)
                    if mode == "stop":
                        self.assertEqual(process.returncode, 0, output)
                    else:
                        self.assertNotEqual(process.returncode, 0)
                        self.assertIn("Java version check timed out", output)
                finally:
                    if pidfile.exists():
                        try:
                            os.killpg(int(pidfile.read_text()), signal.SIGKILL)
                        except ProcessLookupError:
                            pass

    @unittest.skipIf(os.name == "nt", "POSIX executable fixture")
    def test_java_probe_timeout(self):
        java = self.hanging_java_probe()
        for command in ("console", "check"):
            with self.subTest(command=command):
                result = self.command(command, java, "wrapper.startup.timeout=1", check=False, timeout=5)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("Java version check timed out", result.stdout)
                self.assert_probe_stopped()

    @unittest.skipIf(os.name == "nt", "POSIX executable fixture")
    def test_java_probe_remains_manageable(self):
        java = self.hanging_java_probe()
        for cancel in ("signal", "stop"):
            with self.subTest(cancel=cancel):
                for name in ("probe-pid", "probe-child"):
                    (self.root / "boot" / name).unlink(missing_ok=True)
                process = self.console(java, "wrapper.startup.timeout=0")
                self.wait_for(lambda: (self.root / "boot/probe-child").exists())
                self.assertIn("STARTING", self.command("status", timeout=2).stdout)
                if cancel == "signal":
                    process.terminate()
                else:
                    self.command("stop", timeout=3)
                self.assertEqual(process.wait(timeout=3), 0)
                self.assert_probe_stopped()

    @unittest.skipIf(os.name == "nt", "POSIX parent-death behavior")
    def test_parent_death_bounds_hanging_hooks_even_during_stop(self):
        for stopping in (False, True):
            with self.subTest(stopping=stopping):
                before = len(self.events())
                process = self.console("wrapper.java.additional.20=-Dfixture.mode=hang", "wrapper.shutdown.timeout=1")
                self.wait_for(lambda: len(self.events()) > before)
                pid = int(self.events()[before].split()[1])
                try:
                    if stopping:
                        control = (self.root / "bin/fixture.pid.control").read_text().splitlines()
                        with socket.create_connection(("127.0.0.1", int(control[0]))) as connection:
                            connection.sendall(f"{control[1]} stop\n".encode())
                            self.assertEqual(connection.recv(32), b"OK\n")
                        self.wait_for(lambda: "STOPPING" in self.command("status", timeout=2).stdout)
                    process.kill()
                    process.wait(timeout=3)
                    self.wait_for(lambda: not self.process_alive(pid), timeout=5)
                finally:
                    if self.process_alive(pid):
                        os.killpg(pid, signal.SIGKILL)

    @unittest.skipIf(os.name == "nt", "Detached Unix logging")
    def test_detached_diagnostics_follow_rotation(self):
        self.config.write_text(self.source + "wrapper.java.additional.20=-Dfixture.mode=logs\n"
                               + "wrapper.logfile.maxsize=1k\nwrapper.logfile.maxfiles=1\nwrapper.on_exit.0=RESTART\n")
        self.command("start", timeout=10)
        self.wait_for(lambda: (self.root / "logs/console.log.1").exists())
        self.config.write_text(self.config.read_text() + "wrapper.name=invalid name\n")
        self.wait_for(lambda: (self.root / "bin/fixture.pid").read_text() == "")
        logs = "".join(path.read_text() for path in (self.root / "logs").iterdir())
        self.assertIn("wrapper.name must contain", logs)
        self.config.write_text(self.source)

    @unittest.skipIf(os.name == "nt", "Detached Unix logging")
    def test_disabling_file_logging_on_reload_closes_diagnostics(self):
        self.command("start", timeout=10)
        self.wait_for(lambda: len(self.events()) >= 1)
        wrapper_pid = int((self.root / "bin/fixture.pid").read_text())
        self.config.write_text(self.source + "wrapper.logfile.loglevel=NONE\n"
                               + f"wrapper.java.command={self.root}/missing-after-reload\n")
        (self.root / "restart").touch()
        self.wait_for(lambda: (self.root / "bin/fixture.pid").read_text() == "")
        self.wait_for(lambda: not self.process_alive(wrapper_pid))
        self.assertNotIn("missing-after-reload", (self.root / "logs/console.log").read_text())

    @unittest.skipIf(os.name == "nt", "Unix service definitions")
    def test_service_definition_preserves_config_overrides_and_launcher_environment(self):
        launcher = self.root / "bin/service.sh"
        shutil.copy2(PROJECT / "resources/App.sh.in", launcher)
        launcher.chmod(0o755)
        launcher.with_suffix(".shconf").write_text("export FIXTURE_WORD=from-shconf\n")
        custom = self.root / "conf/custom config.conf"
        custom.write_text(self.source + "wrapper.java.additional.20=-Dfixture.mode=once\n")
        result = subprocess.run([str(self.binary), "render-service", str(custom),
                                 "wrapper.name=custom-service", "wrapper.ntservice.password=fixture-secret",
                                 "wrapper.java.additional.10=-Dfixture.value=override %FIXTURE_WORD%",
                                 "--", "two words", 'quote"inside'],
                                capture_output=True, text=True, check=True, timeout=5)
        self.assertNotIn("fixture-secret", result.stdout)
        if platform.system() == "Darwin":
            command = plistlib.loads(result.stdout.encode())["ProgramArguments"]
        else:
            command = shlex.split(next(line.removeprefix("ExecStart=") for line in result.stdout.splitlines()
                                       if line.startswith("ExecStart=")))
            command = [arg.replace("%%", "%").replace("$$", "$") for arg in command]
        # The persisted config must work even when the launcher's default is absent.
        self.config.unlink()
        try:
            launched = subprocess.run(command, capture_output=True, text=True, timeout=10)
            self.assertEqual(launched.returncode, 0, launched.stdout + launched.stderr)
            self.assertIn("property=override from-shconf", launched.stdout)
            self.assertIn("arg=two words", launched.stdout)
            self.assertIn('arg=quote"inside', launched.stdout)
        finally:
            self.config.write_text(self.source)

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

    def test_environment_extensions_survive_configuration_reload(self):
        environment = dict(os.environ, fixture_chain="inherited")
        path_name = "PATH" if os.name == "nt" else next(name for name in environment if name.upper() == "PATH")
        self.config.write_text(self.source
                               + f"set.{path_name}=../custom-bin{os.pathsep}%{path_name}%\n" + '''
set.fixture_alias=%fixture_late%
set.fixture_chain=config:%fixture_chain%
set.default.fixture_chain=ignored
#include.required ../conf/environment.conf
set.default.fixture_late=late
wrapper.java.additional.10=-Dfixture.value=%fixture_chain%/%fixture_alias%
''')
        (self.root / "conf/environment.conf").write_text("set.fixture_chain=include:%fixture_chain%\n")
        process = subprocess.Popen(
            [str(self.binary), "console", str(self.config), "set.fixture_chain=cli:%fixture_chain%"],
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, env=environment)
        self.processes.append(process)
        self.wait_for(lambda: len(self.events()) >= 1)
        (self.root / "restart").touch()
        self.wait_for(lambda: len([e for e in self.events() if e.startswith("start ")]) == 2)
        starts = [e for e in self.events() if e.startswith("start ")]
        self.assertTrue(all(e.endswith("cli:include:config:inherited/late") for e in starts), starts)
        self.command("stop", timeout=10, env=environment)
        self.assertEqual(process.wait(timeout=5), 0)

    def test_environment_name_case_matches_platform(self):
        environment = {name: value for name, value in os.environ.items()
                       if name.upper() != "FIXTURE_CASE"}
        environment["Fixture_Case"] = "inherited"
        self.config.write_text(self.source + '''
set.default.fixture_case=default
set.FIXTURE_CASE=override:%fixture_case%
set.fixture_case=lower:%Fixture_Case%
set.default.Fixture_Case=ignored
wrapper.java.additional.10=-Dfixture.value=%FIXTURE_CASE%/%fixture_case%/%Fixture_Case%
''')
        result = self.command("console", "wrapper.java.additional.20=-Dfixture.mode=once",
                              env=environment, timeout=10)
        expected = "/".join(["lower:override:inherited"] * 3) if os.name == "nt" else (
            "override:default/lower:inherited/inherited")
        self.assertIn("property=" + expected, result.stdout)

    def test_distinct_pid_extensions_have_independent_control(self):
        first_args = ("wrapper.pidfile=../bin/app.pid.1", "wrapper.logfile=../logs/first.log")
        second_args = ("wrapper.pidfile=../bin/app.pid.2", "wrapper.logfile=../logs/second.log")
        first = self.console(*first_args)
        self.wait_for(lambda: len(self.events()) >= 1)
        second = self.console(*second_args)
        self.wait_for(lambda: len(self.events()) >= 2)
        self.assertTrue((self.root / "bin/app.pid.1.control").is_file())
        self.assertTrue((self.root / "bin/app.pid.2.control").is_file())
        self.command("stop", *first_args, timeout=10)
        self.assertEqual(first.wait(timeout=5), 0)
        self.assertIsNone(second.poll())
        self.assertIn("RUNNING", self.command("status", *second_args, timeout=5).stdout)
        self.command("stop", *second_args, timeout=10)
        self.assertEqual(second.wait(timeout=5), 0)
        self.assertEqual(len([e for e in self.events() if e.startswith("stop ")]), 2)

    def test_cyclic_environment_definitions_fail(self):
        for properties in ("set.fixture_cycle=%fixture_cycle%",
                           "set.fixture_cycle=prefix:%fixture_cycle%",
                           "set.fixture_cycle=%fixture_cycle%%fixture_cycle%",
                           "set.fixture_cycle=%fixture_other%\nset.fixture_other=%fixture_cycle%"):
            with self.subTest(properties=properties):
                self.config.write_text(self.source + properties + "\n")
                result = self.command("check", check=False, timeout=5)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("environment expansion", result.stdout.lower())

    @unittest.skipIf(os.name == "nt", "Unix service account")
    def test_service_account_change_requires_wrapper_restart(self):
        process = self.console()
        self.wait_for(lambda: len(self.events()) >= 1)
        try:
            self.config.write_text(self.source + "wrapper.daemon.user=changed-account\n")
            (self.root / "restart").touch()
            output, _ = process.communicate(timeout=10)
            self.assertEqual(process.returncode, 1, output)
            self.assertIn("Changing wrapper.daemon.user or RUN_AS_USER requires stopping", output)
            self.assertEqual(len([e for e in self.events() if e.startswith("start ")]), 1)
            self.assertEqual(len([e for e in self.events() if e.startswith("stop ")]), 1)
        finally:
            self.config.write_text(self.source)

    @unittest.skipIf(os.name == "nt", "Windows start uses SCM; tested separately with service acceptance script")
    def test_detached_start_duplicate_auth_restart_reload_and_stop(self):
        self.command("start", timeout=10)
        self.wait_for(lambda: len(self.events()) >= 1)
        wrapper_pid = (self.root / "bin/fixture.pid").read_text()
        result = self.command("console", check=False, timeout=10)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("already running", result.stdout)
        control = (self.root / "bin/fixture.pid.control").read_text().splitlines()
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
        self.assertIn("RUNNING", self.command("status", timeout=5).stdout)
        (self.root / "stop").touch()
        self.assertEqual(process.wait(timeout=10), 0)
        self.assertTrue(self.events()[-1].startswith("stop "))
        self.assertEqual(self.command("status", check=False, timeout=5).returncode, 3)

    def test_crash_restarts_are_bounded(self):
        result = self.command("console", "wrapper.java.additional.20=-Dfixture.mode=crash", check=False, timeout=15)
        self.assertEqual(result.returncode, 9, result.stdout)
        self.assertEqual(len([e for e in self.events() if e.startswith("start ")]), 2)

    def test_main_failure_follows_exit_policy(self):
        for overrides, invocations in (
            ([], 2),
            (["wrapper.on_exit.1=RESTART", "wrapper.on_exit.default=SHUTDOWN"], 2),
            (["wrapper.on_exit.1=SHUTDOWN"], 1),
            (["wrapper.on_exit.default=SHUTDOWN"], 1),
            (["wrapper.restart.disabled=TRUE"], 1),
        ):
            with self.subTest(overrides=overrides):
                before = len(self.events())
                result = self.command("console", "wrapper.java.additional.20=-Dfixture.mode=throw",
                                      *overrides, check=False, timeout=15)
                self.assertEqual(result.returncode, 1, result.stdout)
                self.assertIn("Fixture main failed", result.stdout)
                events = self.events()[before:]
                self.assertEqual(len([e for e in events if e.startswith("start ")]), invocations)
                # The hook waits for main to return, so failure shutdown must stay asynchronous.
                self.assertEqual(len([e for e in events if e.startswith("stop ")]), invocations)

    def test_main_failure_shutdown_timeout(self):
        result = self.command("console", "wrapper.java.additional.20=-Dfixture.mode=throw-hang",
                              "wrapper.shutdown.timeout=1", check=False, timeout=10)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertEqual(result.stdout.count("Shutdown timeout expired"), 2)
        self.assertEqual(len([e for e in self.events() if e.startswith("start ")]), 2)

    def test_direct_jvm_shutdown_timeout(self):
        for mode in ("once", "exit"):
            with self.subTest(mode=mode):
                before = len(self.events())
                result = self.command("console", "wrapper.java.additional.20=-Dfixture.mode=" + mode,
                                      "wrapper.java.additional.21=-Dfixture.shutdown.delay=60000",
                                      "wrapper.shutdown.timeout=1", check=False, timeout=10)
                self.assertEqual(result.returncode, 1, result.stdout)
                self.assertEqual(result.stdout.count("Shutdown timeout expired"), 2)
                self.assertEqual(len([e for e in self.events()[before:] if e.startswith("start ")]), 2)

    def test_direct_jvm_shutdown_preserves_exit_policy(self):
        for mode, policy, code, invocations in (
            ("once", "wrapper.on_exit.0=RESTART", 1, 2),
            ("exit", "wrapper.on_exit.7=SHUTDOWN", 7, 1),
        ):
            with self.subTest(mode=mode):
                before = len(self.events())
                result = self.command("console", "wrapper.java.additional.20=-Dfixture.mode=" + mode,
                                      "wrapper.java.additional.21=-Dfixture.shutdown.delay=100", policy,
                                      check=False, timeout=10)
                self.assertEqual(result.returncode, code, result.stdout)
                events = self.events()[before:]
                self.assertEqual(len([e for e in events if e.startswith("start ")]), invocations)
                self.assertEqual(len([e for e in events if e.startswith("stop ")]), invocations)

    def test_direct_jvm_shutdown_unlimited(self):
        result = self.command("console", "wrapper.java.additional.20=-Dfixture.mode=exit",
                              "wrapper.java.additional.21=-Dfixture.shutdown.delay=1500",
                              "wrapper.shutdown.timeout=0", "wrapper.on_exit.7=SHUTDOWN",
                              check=False, timeout=10)
        self.assertEqual(result.returncode, 7, result.stdout)
        self.assertEqual(len([e for e in self.events() if e.startswith("stop ")]), 1)
        self.assertNotIn("Shutdown timeout expired", result.stdout)

    def test_explicit_failure_stop_overrides_exit_policy(self):
        result = self.command("console", "wrapper.java.additional.20=-Dfixture.mode=stop",
                              "wrapper.on_exit.1=RESTART", check=False, timeout=10)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertEqual(len([e for e in self.events() if e.startswith("start ")]), 1)
        self.assertTrue(self.events()[-1].startswith("stop "))

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
        for name, arguments in (("service", ["console", "one two"]), ("my-java-app", ["console", "one two"]), ("migrate", ["one two"])):
            path = self.root / "bin" / (name + ".sh")
            launcher = (PROJECT / "resources/App.sh.in").read_text()
            if name == "migrate":
                launcher = launcher.replace(" --script ", " --command ")
            path.write_text(launcher)
            path.chmod(0o755)
            result = subprocess.run([str(path), *arguments], cwd="/", capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("arg=one two", result.stdout)
            if name == "migrate":
                self.assertIn("arg=migrate", result.stdout)

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
        result = self.command("console", "wrapper.restart.disabled=TRUE", "--", "migrate", check=False, timeout=10)
        self.assertEqual(result.returncode, 9, result.stdout)
        self.assertEqual(len(self.events()), 4, self.events())

    def test_generic_and_jsw_bridge_lifecycle(self):
        for main, legacy in (("org.servicewrapper.WrapperSimpleApp", "false"),
                             ("org.tanukisoftware.wrapper.WrapperSimpleApp", "true")):
            with self.subTest(main=main):
                self.config.write_text(self.source + f"wrapper.java.mainclass={main}\n"
                                       + f"wrapper.java.additional.20=-Dfixture.legacy_api={legacy}\n")
                before = len(self.events())
                process = self.console()
                self.wait_for(lambda: len(self.events()) == before + 1)
                (self.root / "restart").touch()
                self.wait_for(lambda: len(self.events()) == before + 3)
                (self.root / "stop").touch()
                self.assertEqual(process.wait(timeout=10), 0)
                self.assertEqual(len(self.events()), before + 4)

    @unittest.skipIf(os.name == "nt", "Unix launcher paths")
    def test_launcher_path_is_generic_and_configurable(self):
        for name in ("service.sh", "my application.sh"):
            path = self.root / "bin" / name
            shutil.copy2(PROJECT / "resources/App.sh.in", path)
            path.chmod(0o755)
        result = self.command("render-service", timeout=5)
        self.assertIn(str(self.root / "bin/service.sh"), result.stdout)
        self.config.write_text(self.source + "wrapper.script=../bin/my application.sh\n")
        result = self.command("render-service", timeout=5)
        self.assertIn(str(self.root / "bin/my application.sh"), result.stdout)

    def test_include_cycle_fails(self):
        self.config.write_text(self.source + "#include.required ../conf/wrapper.conf\n")
        result = self.command("check", check=False, timeout=10)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Recursive", result.stdout)


if __name__ == "__main__":
    unittest.main(verbosity=2)
