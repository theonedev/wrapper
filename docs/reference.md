# Command and configuration reference

[Home](../README.md) · [Integration](integration.md) · [Services](services.md)

## Commands

Use `bin/service.sh COMMAND [application arguments]` on Unix or
`.\bin\service.bat COMMAND [application arguments]` on Windows.
The Bash launcher also supports Git Bash/MSYS on supported Windows architectures.

| Command | Behavior |
| --- | --- |
| `console` | Run in the foreground; Ctrl+C invokes Java shutdown hooks. |
| `install` | Register with the OS service manager. |
| `start` | Start the installed service, or a detached process on unregistered Unix installations. |
| `status` | Print state; exit 0 for active, 3 for stopped/not installed. |
| `stop` | Request Java shutdown and wait for termination. |
| `restart` | Stop, then start. |
| `remove` | Unregister the service; application files remain. |
| `installstart` | Install, then start. |
| `condrestart` | Restart only if already running. |
| `dump` | Unix only: request a JVM thread dump, written to its console output/log. |
| `check` | Check configuration, bridge presence, and Java executable/version without starting the application. |

Arguments after the command are appended to `wrapper.app.parameter.N` values;
the launchers do not interpret them as property overrides. Settings belong in
`conf/wrapper.conf`. Installation saves the selected configuration and launch
arguments; remove/install to change saved arguments. Windows account/password
overrides used at installation are excluded from the saved command line.

Service installation/removal needs administrator/root access. On Windows, service
management requests UAC elevation when required; `console` and `status` never do.
On Unix, use `sudo` to manage installed system services. See
[service administration](services.md) for accounts and permissions.

### Direct binary use

For a custom configuration or temporary property overrides, invoke the native
executable for your platform directly. For example, on Linux amd64:

```sh
boot/wrapper-linux-x86-64 console conf/wrapper.conf wrapper.java.maxmemory=512 -- --port 8080
```

The configuration filename is relative to the caller's directory. Paths inside
it follow the rules below. `--` separates overrides from application arguments.
For Unix installation through the binary, set `wrapper.script` if the launcher
is not at `bin/service.sh`. The launcher normally supplies its own path.

When installed with a `wrapper.pidfile` override, pass that same override to the
binary's `dump` command; it locates the control file directly. To make a utility
launcher, copy a supplied launcher and change `--script` to `--command`: the
filename is prepended to application arguments and file logging is disabled.

## Configuration

The release's `conf/wrapper.conf` documents every supported property with defaults
and optional examples. Its source template is `resources/wrapper.conf`.
Use UTF-8. The file is a properties format, not a shell script.

| Settings | Purpose |
| --- | --- |
| `wrapper.name`, `wrapper.displayname`, `wrapper.description` | Service identity and native registration labels |
| `wrapper.java.command`, `wrapper.java.version.min`, `wrapper.java.mainclass` | Java executable, minimum version, and application entry point |
| `wrapper.java.classpath.N`, `wrapper.java.additional.N`, `wrapper.app.parameter.N` | Classpath entries, JVM options, and application arguments |
| `wrapper.java.initmemory`, `wrapper.java.maxmemory` | Heap sizes in MiB (`0` leaves the JVM option unset) |
| `wrapper.java.library.path.N`, `wrapper.java.library.path.append_system_path` | Native library lookup and optional addition of PATH directories |
| `wrapper.working.dir`, `wrapper.pidfile` | JVM working directory and PID/control-file location |
| `wrapper.daemon.user`, `wrapper.script` | Unix account and launcher |
| `wrapper.ntservice.account`, `wrapper.ntservice.password`, `wrapper.ntservice.dependency.N`, `wrapper.ntservice.starttype` | Windows account, dependencies, and startup policy |
| `wrapper.console.loglevel`, `wrapper.logfile`, `wrapper.logfile.loglevel`, `wrapper.logfile.maxsize`, `wrapper.logfile.maxfiles` | Output destinations and rotation |
| `wrapper.startup.timeout`, `wrapper.startup.wait.timeout`, `wrapper.shutdown.timeout` | JVM startup, command wait, and shutdown limits in seconds; `0` means unlimited |
| `wrapper.restart.delay`, `wrapper.restart.reload_configuration` | Delay and configuration reload for application-requested restarts |

### Values and paths

- Numbered entries are sorted numerically, may have gaps, and each supply one value.
  Spaces inside values are preserved; shell quoting is unnecessary.
- A single `#` starts a comment; `##` inserts a literal hash in a property value.
- Paths default to `boot/`, the native executable's directory. `wrapper.working.dir`
  changes the JVM working directory and base for classpath, native library, log,
  and PID paths. Includes and `wrapper.script` remain relative to `boot/`.
- Classpath filename wildcards `*` and `?` are supported; directories are not
  searched recursively. Native library paths have no wildcard expansion.
- For portable service names, use 1–128 ASCII letters, digits, dots, underscores,
  or hyphens. Each installed instance needs a unique name and separate log/PID paths.

### Environment and includes

```properties
set.APP_MODE=service
set.default.APP_PORT=8080
wrapper.app.parameter.1=--port
wrapper.app.parameter.2=%APP_PORT%
```

`set.NAME` overrides an environment variable; `set.default.NAME` sets it only if
absent. `%NAME%` expands environment values in properties. Assignments may extend
existing variables, such as `set.PATH=../tools:%PATH%` on Unix. References defined
later are resolved after reading the file. Names are case-insensitive on Windows
and case-sensitive on Unix. Cyclic expansions are rejected; expanded values are
limited to 1 MiB.

`#include ../conf/extra.conf` reads an optional file; `#include.required` makes a
missing file an error. These directives are active despite the leading `#`.
Includes use `boot/` as their base, including nested includes. Recursive includes
are rejected. Unix launchers also support `bin/<launcher>.shconf`, `RUN_AS_USER`,
and semicolon-separated `FILES_TO_SOURCE` for shell environment setup.

## Lifecycle behavior

`start` waits for the Java bridge handshake. Java version/executable and JVM-option
failures appear in the terminal and configured log. Application initialization
happens after the handshake; `Started` and `check` do not prove application readiness.
Startup diagnostics retain the last 64 KiB independently of log rotation.
On Unix, a `.startup` result beside the PID file preserves diagnostics after exit.

An ordinary JVM exit, crash, or startup timeout stops Wrapper. Only explicit
Java API restart requests relaunch the JVM; an operator stop takes precedence.
A restart may reload configuration, but changing the Unix account or PID path
requires stopping Wrapper. Registration settings require remove/install.

Operator stops, requested restarts, and normal JVM exit run Java shutdown hooks.
`wrapper.shutdown.timeout` bounds shutdown and may force termination. If the
supervisor connection is lost, the bridge also exits; in that case only, an
unlimited timeout (`0`) uses a 30-second fallback.

Control uses an authenticated loopback connection. Control files append `.control`
to the PID filename and are restricted to the owner on Unix, or the owner,
Administrators, and LocalSystem on Windows. The PID file remains empty after
shutdown to preserve OS lock behavior.

Unix process groups and Windows job objects clean up JVM descendants. Unix helpers
must stay in the inherited process group; a helper that escapes it can survive
cleanup. Inherited stdout/stderr pipes held open by helpers can delay supervisor
shutdown beyond the configured timeout. The Java version probe remains bounded
and cancellable.

## Java Service Wrapper compatibility

Wrapper implements a main-class service lifecycle, with no JNI dependency.
Existing systemd `start sysd`, launchd `launchdinternal`, Windows `-s <config>`,
and launcher `.shconf` conventions remain supported. `wrapper.name` identifies
the service on every platform; stop/remove using the old configuration before
changing it during migration.

Unsupported settings such as `wrapper.on_exit.*`, `wrapper.max_failed_invocations`,
`wrapper.successful_invocation_time`, and `wrapper.restart.disabled` have no effect.
JSW JNI, localization, console formatting/title/input, syslog, CPU/ping monitoring,
auto-bitness, and sequence-gap settings are also inert. Automatic crash restart,
JVM responsiveness monitoring, pause/resume, and arbitrary `WrapperListener`
integrations are not implemented.

Active share/filter/event/timer properties and interactive Windows services fail
with a diagnostic. Other unknown properties are ignored for compatibility;
check spelling against the commented template when a setting has no effect.
