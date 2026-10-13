use crate::{Options, config::Config, supervisor};
use anyhow::{Context, Result, bail, ensure};
use std::{
    ffi::{CStr, CString},
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    process::Command,
};

fn script(config: &Config) -> Result<PathBuf> {
    let path = config
        .environment
        .get("WRAPPER_SCRIPT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            config
                .boot
                .join(config.get("wrapper.script", "../bin/service.sh"))
        });
    fs::canonicalize(path).context("Locating service launcher script")
}

fn definition(config: &Config) -> PathBuf {
    if cfg!(target_os = "macos") {
        PathBuf::from(format!("/Library/LaunchDaemons/{}.plist", config.name()))
    } else {
        PathBuf::from(format!("/etc/systemd/system/{}.service", config.name()))
    }
}
fn init_script(config: &Config) -> PathBuf {
    PathBuf::from(format!("/etc/init.d/{}", config.name()))
}

fn checked(command: &mut Command) -> Result<()> {
    let status = command.status().context("Executing service manager")?;
    ensure!(status.success(), "Service manager failed ({status})");
    Ok(())
}
fn is_loaded(config: &Config) -> bool {
    Command::new("launchctl")
        .args(["print", &format!("system/{}", config.name())])
        .output()
        .is_ok_and(|o| o.status.success())
}

fn launchd_pid(config: &Config) -> Result<Option<u32>> {
    let output = Command::new("launchctl")
        .args(["print", &format!("system/{}", config.name())])
        .output()
        .context("Querying launchd service")?;
    // launchctl returns 113 when the service is not loaded.
    if output.status.code() == Some(113) {
        return Ok(None);
    }
    ensure!(
        output.status.success(),
        "Querying launchd service: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    String::from_utf8(output.stdout)?
        .lines()
        .find_map(|line| line.strip_prefix("\tpid = "))
        .map(|pid| pid.parse().context("Reading launchd service PID"))
        .transpose()
}

fn stop_launchd(config: &Config) -> Result<()> {
    if let Some(pid) = launchd_pid(config)? {
        checked(Command::new("launchctl").args([
            "kill",
            "SIGTERM",
            &format!("system/{}", config.name()),
        ]))?;
        // The wrapper applies its own shutdown timeout, including unlimited waits.
        // Use launchd's PID because installed overrides may change the PID-file path.
        while launchd_pid(config)? == Some(pid) {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    Ok(())
}

fn write_new(path: &PathBuf, contents: &str, mode: u32) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)?;
    if let Err(error) = file
        .write_all(contents.as_bytes())
        .and_then(|_| file.sync_all())
    {
        fs::remove_file(path).with_context(|| {
            format!(
                "Removing incomplete {} after write failed: {error}",
                path.display()
            )
        })?;
        return Err(error.into());
    }
    Ok(())
}

fn launch_args(options: &Options, config: &Config, command: &str) -> Result<Vec<String>> {
    let mut args = vec![
        script(config)?.to_string_lossy().into_owned(),
        "--service".into(),
    ];
    args.extend(options.service_args(command));
    Ok(args)
}

pub fn render(options: &Options, config: &Config) -> Result<String> {
    let config = &runtime_config(options, config)?;
    let args = launch_args(options, config, "console")?;
    if cfg!(target_os = "macos") {
        let mut result = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n<key>Label</key><string>{}</string>\n<key>ProgramArguments</key><array>{}</array>\n<key>RunAtLoad</key><true/>\n<key>KeepAlive</key><false/>\n<key>ExitTimeOut</key><integer>{}</integer>\n",
            xml(config.name()),
            args.iter()
                .map(|arg| format!("<string>{}</string>", xml(arg)))
                .collect::<String>(),
            config.number("wrapper.shutdown.timeout", 30)?
        );
        if !config.user().is_empty() {
            result.push_str(&format!(
                "<key>UserName</key><string>{}</string>\n",
                xml(&config.user())
            ));
        }
        // launchd has a minimal PATH; absolute java.command remains authoritative.
        result.push_str("<key>EnvironmentVariables</key><dict><key>PATH</key><string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string></dict>\n</dict></plist>\n");
        Ok(result)
    } else {
        let start_timeout = config.number("wrapper.startup.wait.timeout", 120)?;
        let start_timeout = if start_timeout == 0 {
            "infinity".into()
        } else {
            start_timeout.to_string()
        };
        let timeout = config.number("wrapper.shutdown.timeout", 30)?;
        let timeout = if timeout == 0 {
            "infinity".into()
        } else {
            (timeout + 10).to_string()
        };
        let mut result = format!(
            "[Unit]\nDescription={}\nAfter=network.target\n\n[Service]\nType=notify\nNotifyAccess=main\nExecStart={}\nKillMode=mixed\nTimeoutStartSec={}\nTimeoutStopSec={}\nRestart=no\n",
            unit_value(config.get("wrapper.description", config.name()))?,
            args.iter()
                .map(|arg| unit_arg(arg))
                .collect::<Result<Vec<_>>>()?
                .join(" "),
            start_timeout,
            timeout
        );
        if !config.user().is_empty() {
            result.push_str(&format!("User={}\n", unit_value(&config.user())?));
        }
        result.push_str("\n[Install]\nWantedBy=multi-user.target\n");
        Ok(result)
    }
}
fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn unit_value(value: &str) -> Result<String> {
    ensure!(
        !value.contains(['\r', '\n', '\0']),
        "Newlines are not allowed in service properties"
    );
    Ok(value.replace('%', "%%").replace('\\', "\\\\"))
}
fn unit_arg(value: &str) -> Result<String> {
    Ok(format!(
        "\"{}\"",
        unit_value(value)?.replace('"', "\\\"").replace('$', "$$")
    ))
}
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn install(options: &Options, config: &Config) -> Result<i32> {
    ensure!(
        unsafe { libc::geteuid() } == 0,
        "Run install/remove as root (sudo)"
    );
    let path = definition(config);
    if path.exists() || init_script(config).exists() {
        println!("{} is already installed", config.name());
        return Ok(0);
    }
    println!("Installing service...");
    let (path, registration) = if cfg!(target_os = "macos")
        || PathBuf::from("/run/systemd/system").is_dir()
    {
        write_new(&path, &render(options, config)?, 0o644)?;
        let registration = (|| {
            if cfg!(target_os = "macos") {
                return Ok(());
            }
            checked(Command::new("systemctl").arg("daemon-reload"))?;
            checked(
                Command::new("systemctl").args(["enable", &format!("{}.service", config.name())]),
            )
        })();
        if registration.is_err() {
            // enable may have created some links before failing.
            let _ = checked(
                Command::new("systemctl").args(["disable", &format!("{}.service", config.name())]),
            );
        }
        (path, registration)
    } else {
        // SysV installations and old JSW init links use the same detached supervisor.
        let args = launch_args(options, config, "start")?;
        let contents = format!(
            "#!/bin/bash\n### BEGIN INIT INFO\n# Provides: {}\n# Required-Start: $remote_fs $network\n# Required-Stop: $remote_fs $network\n# Default-Start: 2 3 4 5\n# Default-Stop: 0 1 6\n# Short-Description: Java service\n### END INIT INFO\nexec {} --service \"$1\" {}\n",
            config.name(),
            shell_quote(&args[0]),
            args[3..]
                .iter()
                .map(|arg| shell_quote(arg))
                .collect::<Vec<_>>()
                .join(" ")
        );
        let path = init_script(config);
        write_new(&path, &contents, 0o755)?;
        let registration = if Command::new("update-rc.d").arg("--help").output().is_ok() {
            let registration =
                checked(Command::new("update-rc.d").args([config.name(), "defaults"]));
            if registration.is_err() {
                let _ = checked(Command::new("update-rc.d").args(["-f", config.name(), "remove"]));
            }
            registration
        } else {
            let registration = checked(Command::new("chkconfig").args(["--add", config.name()]));
            if registration.is_err() {
                let _ = checked(Command::new("chkconfig").args(["--del", config.name()]));
            }
            registration
        };
        (path, registration)
    };
    if let Err(error) = registration {
        fs::remove_file(&path).with_context(|| {
            format!(
                "Removing {} after installation failed: {error:#}",
                path.display()
            )
        })?;
        if path == definition(config) && !cfg!(target_os = "macos") {
            let _ = checked(Command::new("systemctl").arg("daemon-reload"));
        }
        return Err(error);
    }
    println!("Service installed");
    Ok(0)
}

fn native(options: &Options, config: &Config) -> Result<Option<i32>> {
    if options.internal {
        return Ok(None);
    }
    let command = options.command.as_str();
    if !definition(config).exists() {
        let script = init_script(config);
        if !cfg!(target_os = "macos")
            && script.exists()
            && matches!(
                command,
                "start" | "stop" | "restart" | "condrestart" | "status" | "dump"
            )
        {
            // Use saved installation arguments. The legacy marker prevents an
            // init symlink to service.sh from routing straight back here.
            let status = Command::new(script)
                .args([command, "sysd"])
                .status()
                .context("Executing SysV service script")?;
            return Ok(Some(status.code().unwrap_or(1)));
        }
        return Ok(None);
    }
    if cfg!(target_os = "macos") {
        if matches!(command, "start" | "stop" | "restart" | "condrestart") {
            ensure!(
                unsafe { libc::geteuid() } == 0,
                "Installed system service '{}' requires root privileges for '{command}'. Re-run this command with sudo.",
                config.name()
            );
        }
        let target = format!("system/{}", config.name());
        match command {
            "start" => {
                start_native(options, config)?;
            }
            "stop" => {
                println!("Stopping service...");
                // Wait for Java shutdown before unloading, especially for old JSW plists with
                // launchd's default short ExitTimeOut.
                stop_launchd(config)?;
                if is_loaded(config) {
                    checked(Command::new("launchctl").args(["bootout", &target]))?;
                }
                println!("Service stopped");
            }
            "restart" | "condrestart" => {
                if command == "condrestart" && launchd_pid(config)?.is_none() {
                    return Ok(Some(0));
                }
                println!("Stopping service...");
                stop_launchd(config)?;
                println!("Service stopped");
                start_native(options, config)?;
            }
            "status" => {
                let running = launchd_pid(config)?.is_some();
                println!("{}", if running { "RUNNING" } else { "STOPPED" });
                return Ok(Some(if running { 0 } else { 3 }));
            }
            _ => return Ok(None),
        }
    } else {
        match command {
            "start" => start_native(options, config)?,
            "stop" => {
                println!("Stopping service...");
                checked(
                    Command::new("systemctl").args(["stop", &format!("{}.service", config.name())]),
                )?;
                println!("Service stopped");
            }
            "restart" | "condrestart" => {
                if command == "condrestart" && native_pid(config)?.is_none() {
                    return Ok(Some(0));
                }
                println!("Stopping service...");
                checked(
                    Command::new("systemctl").args(["stop", &format!("{}.service", config.name())]),
                )?;
                println!("Service stopped");
                start_native(options, config)?;
            }
            "status" => {
                let output = Command::new("systemctl")
                    .args([
                        "show",
                        "--property=ActiveState",
                        "--value",
                        &format!("{}.service", config.name()),
                    ])
                    .output()
                    .context("Querying systemd service")?;
                ensure!(
                    output.status.success(),
                    "Querying systemd service: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                );
                let output = String::from_utf8(output.stdout)?;
                let state = output.trim();
                ensure!(!state.is_empty(), "Missing systemd service state");
                println!(
                    "{}",
                    match state {
                        "active" => "RUNNING".into(),
                        "inactive" | "failed" => "STOPPED".into(),
                        _ => state.to_ascii_uppercase(),
                    }
                );
                return Ok(Some(if matches!(state, "inactive" | "failed") {
                    3
                } else {
                    0
                }));
            }
            _ => return Ok(None),
        }
    }
    Ok(Some(0))
}

fn native_pid(config: &Config) -> Result<Option<u32>> {
    if cfg!(target_os = "macos") {
        return launchd_pid(config);
    }
    let output = Command::new("systemctl")
        .args([
            "show",
            "--property=MainPID",
            "--value",
            &format!("{}.service", config.name()),
        ])
        .output()
        .context("Querying systemd service PID")?;
    ensure!(
        output.status.success(),
        "Querying systemd service PID: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let pid: u32 = String::from_utf8(output.stdout)?.trim().parse()?;
    Ok((pid != 0).then_some(pid))
}

fn start_native(options: &Options, config: &Config) -> Result<()> {
    let timeout = config.number("wrapper.startup.wait.timeout", 120)?;
    let path = installed_startup_path(options, config)?;
    let pid = native_pid(config)?;
    let startup = crate::startup::FileWait::new(path, pid)?;
    println!("Starting service...");
    if pid.is_none() {
        if cfg!(target_os = "macos") {
            if is_loaded(config) {
                checked(
                    Command::new("launchctl")
                        .args(["kickstart", &format!("system/{}", config.name())]),
                )?;
            } else {
                checked(
                    Command::new("launchctl")
                        .arg("bootstrap")
                        .arg("system")
                        .arg(definition(config)),
                )?;
            }
        } else {
            let result = checked(
                Command::new("systemctl").args(["start", &format!("{}.service", config.name())]),
            );
            if result.is_err() {
                // Type=notify makes systemctl fail before we reach the wait loop.
                // Prefer the new startup report's detailed JVM error when available;
                // pre-exec failures have no report and retain systemctl's diagnostic.
                startup.poll()?;
            }
            result?;
        }
    }
    super::wait_startup(timeout, || startup.poll())
}

// Ask the installed launcher to resolve its paths. This preserves saved property
// overrides, .shconf setup and service-account environment expansion.
fn installed_startup_path(options: &Options, config: &Config) -> Result<PathBuf> {
    let user;
    let mut args = if cfg!(target_os = "macos") {
        let (identity, arguments) = launchd_startup_arguments(&definition(config))?;
        user = identity;
        arguments
    } else {
        let identity = Command::new("systemctl")
            .args([
                "show",
                "--property=User",
                "--value",
                &format!("{}.service", config.name()),
            ])
            .output()?;
        ensure!(
            identity.status.success(),
            "Reading registered service account"
        );
        let identity = String::from_utf8(identity.stdout)?;
        user = if identity.trim().is_empty() {
            "root".into()
        } else {
            identity.trim().into()
        };
        let unit = fs::read_to_string(definition(config))?;
        let command = unit
            .lines()
            .find_map(|line| line.strip_prefix("ExecStart="))
            .context("Missing installed ExecStart")?;
        unit_arguments(command)?
    };
    if args.get(1).is_some_and(|arg| arg == "--service") && args.len() >= 4 {
        args[2] = "startup-path".into();
    } else {
        // Legacy registrations use their launcher's configuration file.
        args = launch_args(options, config, "startup-path")?;
    }
    let output = Command::new(std::env::current_exe()?)
        .args(["--startup-path-as", &user])
        .args(&args)
        .output()
        .context("Resolving installed service startup path")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        let detail = detail.strip_prefix("Wrapper: ").unwrap_or(detail);
        bail!(
            "Cannot start installed service '{}': {}",
            config.name(),
            if detail.is_empty() {
                format!("startup check failed ({})", output.status)
            } else {
                detail.to_owned()
            }
        );
    }
    String::from_utf8(output.stdout)?
        .lines()
        .find_map(|line| line.strip_prefix("WrapperStartupPath="))
        .map(PathBuf::from)
        .context("Missing installed service startup path")
}

fn launchd_startup_arguments(path: &std::path::Path) -> Result<(String, Vec<String>)> {
    // PlistBuddy supports macOS 11; plutil's raw extraction requires macOS 12.
    let property = |key: &str| -> Result<Option<String>> {
        let output = Command::new("/usr/libexec/PlistBuddy")
            .args(["-c", &format!("Print :{key}")])
            .arg(path)
            .output()
            .context("Reading installed launchd arguments")?;
        if output.status.success() {
            let output = String::from_utf8(output.stdout)?;
            // Remove only the reader's newline; an argument may end with one itself.
            return Ok(Some(
                output.strip_suffix('\n').unwrap_or(&output).to_owned(),
            ));
        }
        let error = String::from_utf8_lossy(&output.stderr);
        ensure!(
            error.trim() == format!("Print: Entry, \":{key}\", Does Not Exist"),
            "Reading installed launchd property {key}: {error}"
        );
        Ok(None)
    };
    let mut arguments = Vec::new();
    while let Some(argument) = property(&format!("ProgramArguments:{}", arguments.len()))? {
        ensure!(
            arguments.len() < 4096,
            "Too many installed service arguments"
        );
        arguments.push(argument);
    }
    ensure!(!arguments.is_empty(), "Missing installed ProgramArguments");
    // Missing UserName means root. Resolve the registered identity before any
    // installed launcher or .shconf code is executed.
    let user = property("UserName")?.unwrap_or_else(|| "root".into());
    Ok((user, arguments))
}

fn unit_arguments(command: &str) -> Result<Vec<String>> {
    let mut arguments = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut present = false;
    for ch in command.chars() {
        if escaped {
            ensure!(
                matches!(ch, '\\' | '"' | '\''),
                "Unsupported escape in installed ExecStart"
            );
            current.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
            present = true;
        } else if quote == Some(ch) {
            quote = None;
        } else if quote.is_none() && matches!(ch, '"' | '\'') {
            quote = Some(ch);
            present = true;
        } else if quote.is_none() && ch.is_whitespace() {
            if present {
                arguments.push(
                    std::mem::take(&mut current)
                        .replace("%%", "%")
                        .replace("$$", "$"),
                );
                present = false;
            }
        } else {
            current.push(ch);
            present = true;
        }
    }
    ensure!(
        quote.is_none() && !escaped,
        "Unterminated installed ExecStart argument"
    );
    if present {
        arguments.push(current.replace("%%", "%").replace("$$", "$"));
    }
    Ok(arguments)
}

pub fn manage(options: &Options, config: &Config) -> Result<i32> {
    // Resolve paths as the daemon would, while retaining root's permissions for
    // native service management and removal of installed service definitions.
    let config = &runtime_config(options, config)?;
    if let Some(code) = native(options, config)? {
        return Ok(code);
    }
    match options.command.as_str() {
        "install" => install(options, config),
        "installstart" => {
            install(options, config)?;
            let mut start = options.clone();
            start.command = "start".into();
            manage(&start, config)
        }
        "remove" => {
            ensure!(
                unsafe { libc::geteuid() } == 0,
                "Run install/remove as root (sudo)"
            );
            println!("Removing service...");
            let mut stop = options.clone();
            stop.command = "stop".into();
            let code = manage(&stop, config)?;
            ensure!(
                code == 0,
                "{} could not be stopped (exit code {code}); service registration was retained",
                config.name()
            );
            if definition(config).exists() {
                if !cfg!(target_os = "macos") {
                    checked(
                        Command::new("systemctl")
                            .args(["disable", &format!("{}.service", config.name())]),
                    )?;
                }
                fs::remove_file(definition(config))?;
                if !cfg!(target_os = "macos") {
                    checked(Command::new("systemctl").arg("daemon-reload"))?;
                }
            }
            if init_script(config).exists() {
                if Command::new("update-rc.d").arg("--help").output().is_ok() {
                    checked(Command::new("update-rc.d").args(["-f", config.name(), "remove"]))?;
                } else {
                    checked(Command::new("chkconfig").args(["--del", config.name()]))?;
                }
                fs::remove_file(init_script(config))?;
            }
            println!("Service removed");
            Ok(0)
        }
        "start" => supervisor::start(options, config),
        "stop" => supervisor::stop(config, options.internal),
        "restart" => {
            supervisor::stop(config, options.internal)?;
            supervisor::start(options, config)
        }
        "condrestart" => {
            if supervisor::request(config, "status")?.is_some() {
                let mut restart = options.clone();
                restart.command = "restart".into();
                manage(&restart, config)
            } else {
                Ok(0)
            }
        }
        "status" => match supervisor::request(config, "status")? {
            Some(state) => {
                println!("{state}");
                Ok(0)
            }
            None => {
                println!("STOPPED");
                Ok(3)
            }
        },
        "dump" => {
            ensure!(
                supervisor::request(config, "dump")?.is_some(),
                "Service is not running"
            );
            println!("Thread dump requested, please check console log after a while");
            Ok(0)
        }
        command => bail!("Unknown command: {command}"),
    }
}

pub fn dispatch(_: &Options, _: &Config) -> Result<i32> {
    bail!("The service command is Windows-only; use console under systemd or launchd")
}

struct Account {
    user: String,
    name: CString,
    uid: libc::uid_t,
    gid: libc::gid_t,
    home: String,
}

fn account(config: &Config) -> Result<Option<Account>> {
    let user = config.user();
    if user.is_empty() {
        return Ok(None);
    }
    Ok(Some(account_named(&user)?))
}

fn account_named(user: &str) -> Result<Account> {
    let name = CString::new(user)?;
    unsafe {
        let pwd = libc::getpwnam(name.as_ptr());
        ensure!(!pwd.is_null(), "Unknown service user");
        Ok(Account {
            user: user.to_owned(),
            name,
            uid: (*pwd).pw_uid,
            gid: (*pwd).pw_gid,
            home: CStr::from_ptr((*pwd).pw_dir).to_string_lossy().into_owned(),
        })
    }
}

fn runtime_config(options: &Options, config: &Config) -> Result<Config> {
    let mut environment = std::env::vars().collect::<std::collections::HashMap<_, _>>();
    if let Some(account) = account(config)? {
        environment.insert("HOME".into(), account.home);
        environment.insert("USER".into(), account.user.clone());
        environment.insert("LOGNAME".into(), account.user);
    }
    options.load_with_environment(environment)
}

pub fn switch_user(config: &Config) -> Result<()> {
    let Some(account) = account(config)? else {
        return Ok(());
    };
    switch_account(account)
}

pub fn startup_path_as(user: &str, arguments: &[String]) -> Result<i32> {
    use std::os::unix::process::CommandExt;
    switch_account(account_named(user)?)?;
    Err(Command::new(&arguments[0]).args(&arguments[1..]).exec())
        .context("Executing installed service launcher")
}

fn switch_account(account: Account) -> Result<()> {
    unsafe {
        let current_uid = libc::geteuid();
        let switching = current_uid != account.uid;
        if switching && current_uid != 0 {
            let pwd = libc::getpwuid(current_uid);
            let current_user = if pwd.is_null() {
                format!("UID {current_uid}")
            } else {
                format!(
                    "'{}' (UID {current_uid})",
                    CStr::from_ptr((*pwd).pw_name).to_string_lossy()
                )
            };
            bail!(
                "Service requires user '{}', but this command is running as {current_user}. Run it as '{}' or with sudo.",
                account.user,
                account.user
            );
        }
        // This runs before the supervisor starts any threads. Match runuser/su's
        // account environment so application tools do not use root's home directory.
        std::env::set_var("HOME", account.home);
        std::env::set_var("USER", &account.user);
        std::env::set_var("LOGNAME", &account.user);
        if !switching {
            return Ok(());
        }
        ensure!(
            libc::initgroups(account.name.as_ptr(), account.gid as _) == 0
                && libc::setgid(account.gid) == 0
                && libc::setuid(account.uid) == 0,
            "Unable to switch to service user: {}",
            std::io::Error::last_os_error()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn launchd_startup_arguments_preserve_strings() {
        let path = std::env::temp_dir().join(format!(
            "startup-plist-{}-{}.plist",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let arguments = ["/opt/Java App/$test%/a\"b", "", "a\\b", "汉字\n", "a&<b>"];
        for user in [Some("nobody"), None] {
            fs::write(
                &path,
                format!(
                    "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>ProgramArguments</key><array>{}</array>{}</dict></plist>",
                    arguments.iter().map(|arg| format!("<string>{}</string>", xml(arg))).collect::<String>(),
                    user.map(|user| format!("<key>UserName</key><string>{user}</string>")).unwrap_or_default(),
                ),
            ).unwrap();
            let (identity, actual) = launchd_startup_arguments(&path).unwrap();
            assert_eq!(identity, user.unwrap_or("root"));
            assert_eq!(actual, arguments);
        }
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn service_arguments_are_escaped() {
        assert_eq!(
            unit_arg("/opt/Java App/$test%/a\"b").unwrap(),
            "\"/opt/Java App/$$test%%/a\\\"b\""
        );
        assert!(unit_value("name\nExecStart=evil").is_err());
        assert_eq!(xml("A&B <C>"), "A&amp;B &lt;C&gt;");
        assert_eq!(shell_quote("java'app"), "'java'\\''app'");
        let arguments = [
            "/opt/Java App/$test%/a\"b",
            "two words",
            "a\\b",
            "",
            "汉字",
            "a'bc",
        ];
        let command = arguments
            .iter()
            .map(|arg| unit_arg(arg).unwrap())
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(unit_arguments(&command).unwrap(), arguments);
    }
}
