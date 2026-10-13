#!/usr/bin/env python3
"""Run as root on a disposable Unix host; native service-manager commands are mocked."""
import os
from pathlib import Path
import platform
import plistlib
import pwd
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import time


def alive(pid):
    if not pid:
        return False
    result = subprocess.run(["ps", "-p", str(pid), "-o", "stat="], capture_output=True, text=True)
    return result.returncode == 0 and not result.stdout.strip().startswith("Z")


def manager():
    root = Path(os.environ["WRAPPER_TEST_ROOT"])
    registration = Path(os.environ["WRAPPER_TEST_REGISTRATION"])
    state = root / "manager-pid"
    pid = int(state.read_text()) if state.exists() else 0
    args = sys.argv[1:]
    command = args[0]
    if command == "show":
        if "--property=User" in args:
            print("nobody")
        elif "--property=MainPID" in args:
            print(pid if alive(pid) else 0)
        else:
            print("active" if alive(pid) else "inactive")
    elif command == "print":
        if not state.exists():
            return 113
        if alive(pid):
            print(f"\tpid = {pid}")
    elif command in ("start", "bootstrap", "kickstart"):
        if platform.system() == "Darwin":
            launch = plistlib.loads(registration.read_bytes())["ProgramArguments"]
        else:
            command_line = next(line.removeprefix("ExecStart=") for line in registration.read_text().splitlines()
                                if line.startswith("ExecStart="))
            launch = [arg.replace("%%", "%").replace("$$", "$") for arg in shlex.split(command_line)]
        account = pwd.getpwnam("nobody")
        def demote():
            os.initgroups("nobody", account.pw_gid)
            os.setgid(account.pw_gid)
            os.setuid(account.pw_uid)
        with (root / "native-output").open("ab") as output:
            child = subprocess.Popen(launch, stdin=subprocess.DEVNULL, stdout=output, stderr=output,
                                     start_new_session=True, preexec_fn=demote)
        state.write_text(str(child.pid))
    elif command in ("stop", "kill", "bootout"):
        if alive(pid):
            os.kill(pid, signal.SIGTERM)
            deadline = time.monotonic() + 15
            while alive(pid):
                if time.monotonic() > deadline:
                    return 1
                time.sleep(0.05)
        if command == "bootout":
            state.unlink(missing_ok=True)
    return 0


def test():
    assert os.geteuid() == 0, "Run only as root on a disposable test host"
    project = Path(__file__).resolve().parents[1]
    mac = platform.system() == "Darwin"
    binary_name = "wrapper-macosx-arm-64" if mac else {
        "aarch64": "wrapper-linux-arm-64", "x86_64": "wrapper-linux-x86-64",
    }[platform.machine()]
    name = f"wrapper-startup-test-{os.getpid()}"
    registration = Path(f"/Library/LaunchDaemons/{name}.plist" if mac
                        else f"/etc/systemd/system/{name}.service")
    assert not registration.exists()
    with tempfile.TemporaryDirectory(prefix="native startup ") as temporary:
        root = Path(temporary)
        root.chmod(0o755)
        account = pwd.getpwnam("nobody")
        os.chown(root, account.pw_uid, account.pw_gid)
        for directory in ("boot", "bin", "conf", "logs", "mocks"):
            (root / directory).mkdir()
            os.chown(root / directory, account.pw_uid, account.pw_gid)
        binary = root / "boot" / binary_name
        shutil.copy2(project / "dist/boot" / binary_name, binary)
        shutil.copy2(project / "dist/boot/wrapper.jar", root / "boot/wrapper.jar")
        launcher = root / "bin/service.sh"
        shutil.copy2(project / "resources/App.sh.in", launcher)
        launcher.chmod(0o755)
        launcher.with_suffix(".shconf").write_text(
            'test "$(id -u)" != 0 || { echo "Launcher ran as root" >&2; exit 1; }\n')
        subprocess.run(["javac", "--release", "17", "-cp", str(root / "boot/wrapper.jar"),
                        "-d", str(root / "boot"), str(project / "tests/Fixture.java")], check=True)
        config = root / "conf/wrapper.conf"
        source = f"""wrapper.name={name}
wrapper.java.command={shutil.which('java')}
wrapper.java.classpath.1=.
wrapper.app.parameter.1=Fixture
wrapper.java.additional.1=-Dfixture.root={root}
wrapper.restart.delay=0
wrapper.shutdown.timeout=3
wrapper.startup.wait.timeout=10
"""
        config.write_text(source)
        for manager_name in ("systemctl", "launchctl"):
            mock = root / "mocks" / manager_name
            mock.write_text(f"#!{sys.executable}\nimport runpy, sys\nnamespace = runpy.run_path({str(Path(__file__).resolve())!r}, run_name='service_manager_mock')\nsys.exit(namespace['manager']())\n")
            mock.chmod(0o755)
        environment = dict(os.environ, PATH=f"{root}/mocks:{os.environ['PATH']}",
                           WRAPPER_TEST_ROOT=str(root), WRAPPER_TEST_REGISTRATION=str(registration))
        def invoke(command, **kwargs):
            return subprocess.run([str(binary), command, str(config)], env=environment,
                                  capture_output=True, text=True, timeout=20, **kwargs)
        rendered = subprocess.run([str(binary), "render-service", str(config), "wrapper.daemon.user=nobody",
                                   "wrapper.pidfile=../bin/installed-%USER%.pid",
                                   "wrapper.logfile=../logs/installed-%USER%.log"],
                                  capture_output=True, text=True, check=True)
        registration.parent.mkdir(parents=True, exist_ok=True)
        registration.write_text(rendered.stdout)
        try:
            for logging in ("INFO", "NONE"):
                for setting, diagnostic in (
                    (f"wrapper.java.command={root}/missing-java", "missing-java"),
                    ("wrapper.java.additional.2=-Xmxinvalid-fixture-size", "invalid-fixture-size"),
                    ("wrapper.java.version.min=999", "wrapper.java.version.min"),
                ):
                    config.write_text(source + f"wrapper.logfile.loglevel={logging}\n{setting}\n")
                    result = invoke("start")
                    output = result.stdout + result.stderr
                    assert result.returncode != 0 and diagnostic in output, output
                    assert "Starting service..." in output and "Service started" not in output, output
                    assert (root / "bin/installed-nobody.pid.startup").stat().st_mode & 0o777 == 0o600
                    invoke("stop", check=True)
            slow_java = root / "boot/slow-java"
            slow_java.write_text(f"#!/bin/sh\nsleep 0.5\nexec {shlex.quote(shutil.which('java'))} \"$@\"\n")
            slow_java.chmod(0o755)
            config.write_text(source + f"wrapper.java.command={slow_java}\n")
            started = time.monotonic()
            result = invoke("start", check=True)
            assert result.stdout == "Starting service...\nService started\n", result.stdout
            assert time.monotonic() - started >= 1
            invoke("start", check=True)
            invoke("restart", check=True)
            invoke("stop", check=True)
            invoke("condrestart", check=True)
            status = invoke("status")
            assert (status.returncode, status.stdout) == (3, "STOPPED\n"), status
            assert not (root / "bin" / f"{name}.pid.startup").exists()
            print(f"{platform.system()} native startup tests passed (mock service manager)")
        finally:
            invoke("stop")
            registration.unlink(missing_ok=True)


if __name__ == "__main__":
    if Path(sys.argv[0]).name in ("systemctl", "launchctl"):
        sys.exit(manager())
    test()
