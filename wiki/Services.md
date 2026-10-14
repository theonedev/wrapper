# Manage services

[Home](Home.md) · [Integration](Integration.md) · [Reference](Configuration.md)

First test the application with `console`. Place it at its permanent location,
set a unique `wrapper.name`, and select Java with `wrapper.java.command`.
Prefer an absolute Java path: services do not load your shell profile.

## Install and start

On macOS/Linux, from the application directory:

```sh
sudo bin/service.sh install
sudo bin/service.sh start
bin/service.sh status
```

On Windows, from Command Prompt or PowerShell:

```bat
.\bin\service.bat install
.\bin\service.bat start
.\bin\service.bat status
```

Windows requests UAC elevation when needed. On Unix, managing an installed
system service requires root; `status` can run without it. Use `stop`, `restart`,
or `remove` in place of `start` for those operations. Stop before removing a
service. Removal unregisters it; your application files remain in place.

| Platform | Registration | Native inspection |
| --- | --- | --- |
| Linux | systemd unit `<wrapper.name>.service`; SysV fallback when systemd is not running | `systemctl status my-app.service`, `journalctl -u my-app.service` |
| macOS | System launchd daemon in `/Library/LaunchDaemons/`; label is `wrapper.launchd.label` (defaults to `wrapper.name`) | `sudo launchctl print system/my-app` |
| Windows | Windows SCM service named `wrapper.name` | Services app or `sc.exe query my-app` |

On Unix, `start` without registration runs a detached process; it does not enable
startup at boot. Windows requires installation before `start`. `installstart`
combines installation and startup; `condrestart` restarts only an active instance.
See the [command reference](Configuration.md#commands) for the full list.

## macOS launchd label

Set `wrapper.launchd.label=com.example.my-app` to choose a macOS label independently
of `wrapper.name`. This registers `/Library/LaunchDaemons/com.example.my-app.plist`
and uses `system/com.example.my-app` for launchctl operations. The value accepts
1 to 128 ASCII letters, digits, dots, underscores or hyphens. When omitted, the
label remains `wrapper.name`. Linux and Windows ignore this property; default
PID/control filenames still use `wrapper.name` on every platform.

For an existing service, stop and remove it using its old label before changing
this setting, then install and start it again. Changing the setting does not
rename an existing registration. To retain a JSW registration during migration,
set this property to its existing launchd label, including any JSW prefix.

## Service accounts

On Unix, set `wrapper.daemon.user=myapp` before installation to use an existing
account. The installer does not create accounts. Otherwise, an installed system
service normally runs as root. macOS registers system daemons, so management
still requires root when the configured account is your own user.

On Windows, the default account is LocalSystem. Set `wrapper.ntservice.account`
and `wrapper.ntservice.password` before installation to use another account;
it must have the right to log on as a service. Restrict access to configuration
containing credentials. After installation, SCM stores the credential; clear the
plaintext password from `wrapper.conf` and supply it again if reinstalling.
Use local paths; services do not inherit an interactive user's mapped drives.

The account needs read/execute access to Java and application files, plus write
access to application data, logs, and PID/control-file directories. By default,
Wrapper writes logs under `logs/` and PID/control files under `bin/`. For a
read-only installation, set `wrapper.logfile` and `wrapper.pidfile` to writable
locations. Keep configuration and executables writable only by trusted administrators.
Services have no interactive terminal; stdin is closed and GUI prompts are unsuitable.

Windows startup defaults to `wrapper.ntservice.starttype=AUTO_START`.
`DELAY_START` and `DEMAND_START` are also supported. Numbered
`wrapper.ntservice.dependency.N` entries identify Windows services or load-order
groups to start first. Reinstall after changing registration settings.

## Windows network shares

Use `wrapper.share.N.*` to map a network drive or printer in the same logon session
as the wrapper and JVM. It works in console and installed service mode. A drive
mapped in an interactive desktop session may not be visible to a Windows service.

```properties
wrapper.share.1.location=\\fileserver\data
wrapper.share.1.target=Z:
wrapper.share.1.type=DISK
wrapper.share.1.startup.failure=SHUTDOWN
wrapper.share.1.startup.premapped=SHUTDOWN
wrapper.share.1.shutdown.unmap=TRUE
```

The default uses the process account's network credentials. For separate share
credentials, set `wrapper.share.1.account` and `wrapper.share.1.password` in a
protected configuration file; environment references such as `%SHARE_PASSWORD%`
are supported. These credentials are needed at each wrapper startup, unlike the
SCM service-account password used only for installation. Network provider and
service-account restrictions still apply, including conflicting credentials for
connections to the same server. Credentials are passed directly to Windows APIs,
without a command shell or interactive authentication prompt.

| Property suffix | Behavior and default |
| --- | --- |
| `location`, `target` | Required UNC path and drive letter (`Z:`) or printer port (`LPT1`). |
| `type` | `ANY` (default, inferred from target), `DISK` or `PRINTER`. |
| `account`, `password` | Optional credentials. Password is ignored without an account. |
| `startup.failure` | `CONTINUE` (default) logs a mapping failure and launches Java; `SHUTDOWN` fails startup. |
| `startup.premapped` | Separate `CONTINUE`/`SHUTDOWN` policy for an occupied target; default `CONTINUE`, including when the existing mapping matches. |
| `startup.max_retries` | Total attempts for temporary network failures; default 5, with 0 allowing one attempt. Permanent errors are not retried. |
| `startup.retry_interval` | Seconds between attempts; default 10. Stop requests interrupt the wait, but cannot interrupt an in-flight Windows networking call. |
| `shutdown.unmap` | Default `FALSE`. When `TRUE`, disconnect mappings created by this wrapper after the JVM and its shutdown hooks finish, including on startup failure. |
| `shutdown.unmap.force` | Default `FALSE`. Permit disconnection despite open files/jobs. |

Existing mappings are never replaced or taken over for cleanup. Keep configuration,
wrapper logs and PID/control files local: they are opened before mapping starts.
Mappings survive JVM restarts, including configuration reloads; restart the entire
wrapper to change share settings. `check` validates settings without mapping shares.
Active share settings are rejected on Linux/macOS.

Use `bin/service.bat stop` to interrupt share retries during service startup.
Windows does not accept stop controls from the Services app or `sc.exe stop`
while the service is in `StartPending`.

## Logs and troubleshooting

Application stdout/stderr go to `logs/console.log` by default. Configure rotation
with `wrapper.logfile.maxsize` and `wrapper.logfile.maxfiles`. A destination's
log level of `NONE` disables it; other levels forward output without filtering.
Use your application's logging framework to filter individual messages.

| Symptom | Check |
| --- | --- |
| Java cannot be found | Set an absolute `wrapper.java.command`; check Java 17+ and your application's required version. |
| `ClassNotFoundException` | Check the main class and classpath. Executable JARs use the manifest's `Main-Class`. |
| Missing configuration or data files | Paths default to `boot/`; see [working directory](Integration.md#working-directory). |
| `Started` followed by exit | Startup confirms the bridge handshake, not application readiness. Read the logs and ensure a non-daemon thread remains alive. |
| Permission denied | Check the service account's directory traversal, read/execute, and log/PID/data write permissions. |
| Application stays stopped after a crash | Ordinary JVM exits stop Wrapper; automatic crash retry is not implemented. |
| Shutdown hangs or is forced | Check shutdown hooks and `wrapper.shutdown.timeout`; `0` allows unlimited normal shutdown. |
| Need a thread dump | Run `bin/service.sh dump` on Unix and read the console log. Windows does not support this command. |

Startup diagnostics also appear in the initiating terminal, even when file logging
is disabled. `status` reports process/service state, returning 0 for active and
3 for stopped/not installed; use an application-specific health endpoint for readiness.

## Upgrades and removal

Stop the service before replacing application JARs or Wrapper files. Preserve
local configuration, data, and a copy of the previous version for rollback.
Keep `wrapper.jar` and native executables from the same release together.

For upgrades at the same location with unchanged registration settings, replace
files while stopped and start again. If changing the path, name, account,
dependencies, description, startup policy, or installed launch arguments, stop
and remove the old service using its original configuration, then reinstall it.
Editing `wrapper.conf` alone does not update native service registration.

Remove a service before deleting its directory or account. Wrapper does not
migrate application data or manage application-specific upgrade steps.
