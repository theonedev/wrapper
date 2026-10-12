# ServiceWrapper

A Rust service supervisor for Java applications, with Bash and Windows batch
launchers. It installs and manages services on Windows, macOS, and Linux, launches
Java directly, rotates output logs, and restarts failed JVMs. JVM options and
service settings are configured in `conf/wrapper.conf`.

This standalone project is maintained at
[code.onedev.io/onedev/servicewrapper](https://code.onedev.io/onedev/servicewrapper).
It has no OneDev application dependency. OneDev server/agent packaging is an
optional integration; other Java applications use the generic distribution.

## Use with a Java application

Extract `servicewrapper-VERSION.tar.gz`, put your application jars in `lib`, and
edit `conf/wrapper.conf`:

```properties
wrapper.name=my-app
wrapper.displayname=My Application
wrapper.java.mainclass=com.example.Main
wrapper.java.classpath.1=../lib/*.jar
wrapper.java.additional.1=-Xmx1g
wrapper.app.parameter.1=--port
wrapper.app.parameter.2=8080
```

Run `bin/service.sh console` (Windows: `bin\service.bat console`) first, then
`install` and `start` to register and run a service. The application only needs a
normal `public static void main(String[] args)` entry point; no wrapper API is
required. Java shutdown hooks run when the service stops. Scripts can be renamed,
for example to `my-app.sh` and `my-app.bat`; the service name comes from the config.
Prefer `wrapper.app.parameter.N` for arguments that should also apply at boot.

Applications that need to request their own restart or stop can compile against
`boot/wrapper.jar` and call `io.onedev.servicewrapper.WrapperManager.restartAndReturn()`
or `stopAndReturn(int)`. The equivalent `org.tanukisoftware.wrapper.WrapperManager`
methods and `WrapperSimpleApp` main class remain available for JSW compatibility.
The bridge is independently implemented and contains no Tanuki code or JNI library.
See the compatibility limits below before migrating existing JSW integrations.

## Platforms

| Platform | Rust target | Distribution executable |
| --- | --- | --- |
| Windows amd64 | `x86_64-pc-windows-msvc` | `wrapper-windows-x86-64.exe` |
| Windows arm64 | `aarch64-pc-windows-msvc` | `wrapper-windows-arm-64.exe` |
| Apple Silicon | `aarch64-apple-darwin` | `wrapper-macosx-arm-64` |
| Linux amd64 | `x86_64-unknown-linux-musl` | `wrapper-linux-x86-64` |
| Linux arm64 | `aarch64-unknown-linux-musl` | `wrapper-linux-arm-64` |

Linux executables are statically linked with musl, and Windows executables use the
static C runtime. The Java application still needs Java 17 or newer. The old
`wrapper-macosx-universal-64` filename is also supplied in the OneDev profile as an Apple Silicon alias;
Intel macOS and 32-bit systems are not supported.

## Commands

Generic entry points are `bin/service.sh` on Unix and `bin/service.bat` on Windows.
The OneDev integration retains `bin/server.sh` / `bin/agent.sh` and their `.bat` counterparts. The Bash launcher also recognizes
Git Bash/MSYS on supported Windows architectures.

```sh
bin/service.sh install
bin/service.sh start
bin/service.sh status
bin/service.sh restart
bin/service.sh stop
bin/service.sh remove
bin/service.sh console
```

`installstart` installs and starts. `condrestart` restarts only a running instance.
Unix additionally supports `dump` to request a JVM thread dump. Application arguments preserve boundaries, spaces, and exit codes. Additional
arguments do not disable service crash recovery. To create a one-shot command
launcher, change its `--script` argument to `--command`: its filename is prepended
to the application's arguments and crash retries are disabled. Direct invocation
also accepts `wrapper.restart.disabled=TRUE`. OneDev's existing Docker maintenance
entry point `boot/wrapper-linux-* conf/wrapper.conf -- upgrade ...` is supported.

Service registration uses Windows SCM, Linux systemd (or SysV when systemd is not
running), and macOS launchd. Installation/removal require administrator/root access;
ordinary console and detached Unix operation do not. Unix `start` without service
registration starts a detached supervisor. Windows `start` requires installation.
Status returns 0 for an active instance and 3 when stopped/not installed.

Existing systemd units invoking `server.sh start sysd` and launchd plists invoking
`server.sh launchdinternal` remain supported. macOS retains the existing
`org.tanukisoftware.wrapper.<name>` service label. The Windows `-s <config>` service
entry point and amd64 executable filename remain unchanged. Configuration filenames,
boot-relative paths, `.shconf`, `RUN_AS_USER`, and `FILES_TO_SOURCE` remain supported.

Stop a deployment before replacing its wrapper files. OneDev-specific packaging
and existing-agent migration are documented in [the OneDev integration documentation](https://code.onedev.io/onedev/servicewrapper/~files/main/integrations/onedev/README.md).

## Configuration compatibility

`conf/wrapper.conf` remains a UTF-8 properties file, not a shell script. Supported
settings include:

- `wrapper.java.command`, `wrapper.java.version.min`, `wrapper.java.mainclass`.
- Numbered `wrapper.java.classpath.N`, `wrapper.java.library.path.N`,
  `wrapper.java.additional.N`, and `wrapper.app.parameter.N`. Entries are sorted
  numerically and may have gaps. Classpath wildcards apply to filenames.
- `wrapper.java.library.path.append_system_path`, `wrapper.java.initmemory`,
  `wrapper.java.maxmemory`, and `wrapper.working.dir`.
- `%ENVIRONMENT_VARIABLE%`, `set.NAME`, `set.default.NAME`, optional `#include`,
  and `#include.required`. Include paths are relative to `boot`, as in the existing
  OneDev configuration. Recursive includes are rejected. `##` represents a literal
  hash in property values; a single hash begins a comment.
- `wrapper.name`, `wrapper.displayname`, `wrapper.description`, and Unix
  `wrapper.daemon.user` (or `RUN_AS_USER` in the launcher/`.shconf`). Unix service
  names use letters, digits, dots, underscores, and hyphens. `wrapper.script` supplies
  the Unix launcher path when installing through the binary directly (default
  `../bin/service.sh`); scripts supply their own path automatically.
- Windows `wrapper.ntservice.account`, `wrapper.ntservice.password`, numbered
  `wrapper.ntservice.dependency.N`, and `wrapper.ntservice.starttype` with
  `AUTO_START`, `DELAY_START`, or `DEMAND_START`. A custom account must have the
  Windows right to log on as a service. Service registration changes require
  remove/install, just as before.
- `wrapper.logfile`, `wrapper.logfile.maxsize` (bytes, `k`, or `m`), and
  `wrapper.logfile.maxfiles` (0 keeps all rotated files).
  `wrapper.console.loglevel=NONE` and `wrapper.logfile.loglevel=NONE` disable their
  destinations. Other levels preserve the application's unmodified stdout/stderr;
  application filtering remains in your application's logging configuration.
- `wrapper.startup.timeout`, `wrapper.shutdown.timeout` (0 means unlimited),
  `wrapper.restart.delay`, `wrapper.max_failed_invocations`,
  `wrapper.successful_invocation_time`, `wrapper.restart.reload_configuration`,
  `wrapper.on_exit.<code>` and `wrapper.on_exit.default` (`RESTART`/`SHUTDOWN`).
  Normal exit defaults to shutdown; a crash defaults to restart. Explicit Java API
  stop/restart requests take precedence. `wrapper.restart.disabled=TRUE` disables retries for one-shot commands.
- `wrapper.pidfile` and `wrapper.startup.wait.timeout` (default 120 seconds for
  detached startup). The PID file is retained empty after shutdown so its OS lock
  cannot be bypassed by racing unlink/recreation.

The wrapper implements the Java main-class service lifecycle, not the entire JSW product.
JSW license, JNI, localization, console formatting/title, syslog, CPU/ping monitor,
auto-bitness, sequence-gap, and console-input settings are inert compatibility
entries. In particular, no JVM responsiveness/ping watchdog is implemented;
OneDev's existing configuration disables it. Network-share mappings, output filters,
event/timer actions, interactive services, pause/resume, and arbitrary WrapperListener
integrations are not implemented. Active share/filter/event/timer properties and
interactive services fail with a diagnostic instead of being silently accepted.
Other unknown properties are ignored for compatibility. Users depending on advanced
JSW features should review this list before migration.

The supervisor launches Java directly, without evaluating JVM options in a shell.
Control uses an authenticated loopback connection. The token file is owner-only on
Unix and restricted to its owner, Administrators, and LocalSystem on Windows.
Stopping invokes Java shutdown hooks; configured timeouts force termination when
necessary. Unix process groups and Windows job objects clean up JVM descendants.
The Java bridge also initiates shutdown when its supervisor connection is lost.
Crash retries are bounded, and control remains available during restart delays.

## Build and package

Use Rust 1.96.1, JDK 17+, and Python 3.11+. `Cargo.lock` fixes dependency versions.
For cross-compilation install Zig 0.16.0, `cargo-zigbuild` 0.23.4, and `cargo-xwin`
0.23.1. Windows cross-compilation also needs LLVM's `clang-cl` and `lld-link`;
`cargo-xwin` downloads the Microsoft SDK/CRT. Native Windows builds use MSVC tools.
macOS builds run natively on Apple Silicon, or cross-compile from Linux with Zig
and a macOS SDK (`SDKROOT`; defaults to `/opt/MacOSX11.3.sdk` on Linux).

```sh
rustup target add aarch64-apple-darwin \
  x86_64-unknown-linux-musl aarch64-unknown-linux-musl \
  x86_64-pc-windows-msvc aarch64-pc-windows-msvc
./build.sh aarch64-apple-darwin \
  x86_64-unknown-linux-musl aarch64-unknown-linux-musl \
  x86_64-pc-windows-msvc aarch64-pc-windows-msvc
python3 package.py --archive dist/artifacts
python3 package.py --profile onedev --archive dist/artifacts
```

`build.sh` without arguments builds for the host (using a static musl target on
Linux). Outputs go to `dist/boot`. `package.py` requires all five
binaries, verifies their architecture, collects dependency license texts in
`wrapper.jar`, and writes `SHA256SUMS`. Its generic output is `dist/release/generic`;
the `onedev` profile produces Maven packaging inputs in `dist/release/onedev`.
Use `--destination` to choose another output directory. `--archive` additionally
writes a versioned archive and SHA-256 file to the supplied directory.
Vendored license texts include pinned provenance in `licenses/sources.json`.
Build caches and binaries are ignored; release artifacts are published by CI.

The OneDev **Release** job checks formatting, runs Rust tests and clippy, builds all
five targets, runs JVM integration and service-account tests on its Linux host,
and publishes both archives through a `PublishArtifactStep`. It uses `1dev/buildenv`
and runs manually with `tod build run Release --branch main`. The buildenv project
must first publish the image containing Rust 1.96.1, Zig, cargo-zigbuild,
cargo-xwin, LLVM, and the macOS SDK. Windows SDK/CRT downloads require network
access during the first cross-build. No CI step installs tools into a later step's
isolated environment, and no application source is needed to produce the wrapper.

## Verification

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
python3 tests/integration.py
bash tests/linux-smoke.sh
# After building the server release (also checks its generated agent distribution):
python3 integrations/onedev/check_package.py /path/to/server-product/target/onedev-VERSION.zip /path/to/server-product/wrapper
```

The integration suite requires localhost sockets and launches only temporary JVM
fixtures. It checks configuration, quoting, shutdown hooks, forced shutdown, startup
timeouts, duplicate starts, authenticated control, restarts with configuration reload,
parent death, bounded crash retries, log rotation, and launcher/maintenance arguments.
The Linux smoke test also runs in a disposable Java 17 container with this project
mounted read-only. Run each architecture's binary on its corresponding host or emulator.

`tests/service-unix.sh` and `tests/service.ps1` test actual OS service installation,
restart, shutdown hooks, and removal using a unique fixture service. Run these as
administrator/root on disposable hosts, never against a production instance.

Validated during this implementation: all five release binaries built; five Rust
unit tests and eleven macOS JVM integration tests passed; Linux arm64 and amd64
lifecycle and service-account tests passed in containers (amd64 under emulation).
SysV service installation, restart, shutdown, and removal passed in a disposable
Linux container. The Maven plugin and server compiled, and clean and incremental
release packages passed binary, permission, configuration, and obsolete-file checks.
Native SCM, launchd, and systemd installation acceptance is a release gate and has
not been run on this development machine.

Service APIs follow the primary documentation for
[Windows services](https://docs.rs/windows-service/0.8.1/windows_service/),
[systemd services](https://www.freedesktop.org/software/systemd/man/latest/systemd.service.html),
and [launchd jobs](https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPSystemStartup/Chapters/CreatingLaunchdJobs.html).
