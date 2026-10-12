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
        .get("SERVICEWRAPPER_SCRIPT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            config
                .boot
                .join(config.get("wrapper.script", "../bin/service.sh"))
        });
    fs::canonicalize(path).context("Locating service launcher script")
}

fn label(config: &Config) -> String {
    // Keep JSW's label so existing LaunchDaemons remain manageable after an upgrade.
    format!(
        "{}.{}",
        config
            .environment
            .get("PLIST_DOMAIN")
            .map(String::as_str)
            .unwrap_or("org.tanukisoftware.wrapper"),
        config.name()
    )
}
fn definition(config: &Config) -> PathBuf {
    if cfg!(target_os = "macos") {
        PathBuf::from(format!("/Library/LaunchDaemons/{}.plist", label(config)))
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
        .args(["print", &format!("system/{}", label(config))])
        .output()
        .is_ok_and(|o| o.status.success())
}

fn write_new(path: &PathBuf, contents: &str, mode: u32) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

pub fn render(config: &Config) -> Result<String> {
    let script = script(config)?;
    if cfg!(target_os = "macos") {
        let mut result = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n<key>Label</key><string>{}</string>\n<key>ProgramArguments</key><array><string>{}</string><string>console</string></array>\n<key>RunAtLoad</key><true/>\n<key>KeepAlive</key><false/>\n<key>ExitTimeOut</key><integer>{}</integer>\n",
            xml(&label(config)),
            xml(&script.to_string_lossy()),
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
        let timeout = config.number("wrapper.shutdown.timeout", 30)?;
        let timeout = if timeout == 0 {
            "infinity".into()
        } else {
            (timeout + 10).to_string()
        };
        let mut result = format!(
            "[Unit]\nDescription={}\nAfter=network.target\n\n[Service]\nType=simple\nExecStart={} console\nKillMode=mixed\nTimeoutStopSec={}\nRestart=no\n",
            unit_value(config.get("wrapper.description", config.name()))?,
            unit_arg(&script.to_string_lossy())?,
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

fn install(config: &Config) -> Result<i32> {
    ensure!(
        unsafe { libc::geteuid() } == 0,
        "Run install/remove as root (sudo)"
    );
    let path = definition(config);
    if path.exists() || init_script(config).exists() {
        println!("{} is already installed", config.name());
        return Ok(0);
    }
    if cfg!(target_os = "macos") || PathBuf::from("/run/systemd/system").is_dir() {
        write_new(&path, &render(config)?, 0o644)?;
        if !cfg!(target_os = "macos") {
            checked(Command::new("systemctl").arg("daemon-reload"))?;
            checked(
                Command::new("systemctl").args(["enable", &format!("{}.service", config.name())]),
            )?;
        }
    } else {
        // SysV installations and old JSW init links use the same detached supervisor.
        let contents = format!(
            "#!/bin/bash\n### BEGIN INIT INFO\n# Provides: {}\n# Required-Start: $remote_fs $network\n# Required-Stop: $remote_fs $network\n# Default-Start: 2 3 4 5\n# Default-Stop: 0 1 6\n# Short-Description: Java service\n### END INIT INFO\nexec {} \"$1\" sysd\n",
            config.name(),
            shell_quote(&script(config)?.to_string_lossy())
        );
        write_new(&init_script(config), &contents, 0o755)?;
        if Command::new("update-rc.d").arg("--help").output().is_ok() {
            checked(Command::new("update-rc.d").args([config.name(), "defaults"]))?;
        } else {
            checked(Command::new("chkconfig").args(["--add", config.name()]))?;
        }
    }
    println!("{} installed", config.name());
    Ok(0)
}

fn native(options: &Options, config: &Config) -> Result<Option<i32>> {
    if options.internal || !definition(config).exists() {
        return Ok(None);
    }
    let command = options.command.as_str();
    if cfg!(target_os = "macos") {
        let target = format!("system/{}", label(config));
        match command {
            "start" => {
                if is_loaded(config) {
                    checked(Command::new("launchctl").args(["kickstart", &target]))?;
                } else {
                    checked(
                        Command::new("launchctl")
                            .arg("bootstrap")
                            .arg("system")
                            .arg(definition(config)),
                    )?;
                }
            }
            "stop" => {
                // Wait for Java shutdown before unloading, especially for old JSW plists with
                // launchd's default short ExitTimeOut.
                supervisor::stop(config)?;
                if is_loaded(config) {
                    checked(Command::new("launchctl").args(["bootout", &target]))?;
                }
            }
            "restart" => {
                supervisor::stop(config)?;
                if is_loaded(config) {
                    checked(Command::new("launchctl").args(["kickstart", &target]))?;
                } else {
                    checked(
                        Command::new("launchctl")
                            .arg("bootstrap")
                            .arg("system")
                            .arg(definition(config)),
                    )?;
                }
            }
            _ => return Ok(None),
        }
    } else {
        match command {
            "start" | "stop" | "restart" => checked(
                Command::new("systemctl").args([command, &format!("{}.service", config.name())]),
            )?,
            _ => return Ok(None),
        }
    }
    Ok(Some(0))
}

pub fn manage(options: &Options, config: &Config) -> Result<i32> {
    if let Some(code) = native(options, config)? {
        return Ok(code);
    }
    match options.command.as_str() {
        "install" => install(config),
        "installstart" => {
            install(config)?;
            let mut start = options.clone();
            start.command = "start".into();
            manage(&start, config)
        }
        "remove" => {
            ensure!(
                unsafe { libc::geteuid() } == 0,
                "Run install/remove as root (sudo)"
            );
            let mut stop = options.clone();
            stop.command = "stop".into();
            manage(&stop, config)?;
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
            println!("{} removed", config.name());
            Ok(0)
        }
        "start" => supervisor::start(options, config),
        "stop" => supervisor::stop(config),
        "restart" => {
            supervisor::stop(config)?;
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
                println!("{}: {state}", config.name());
                Ok(0)
            }
            None => {
                println!("{}: STOPPED", config.name());
                Ok(3)
            }
        },
        "dump" => {
            ensure!(
                supervisor::request(config, "dump")?.is_some(),
                "Service is not running"
            );
            Ok(0)
        }
        command => bail!("Unknown command: {command}"),
    }
}

pub fn dispatch(_: &Options, _: &Config) -> Result<i32> {
    bail!("The service command is Windows-only; use console under systemd or launchd")
}

pub fn switch_user(config: &Config) -> Result<()> {
    let user = config.user();
    if user.is_empty() {
        return Ok(());
    }
    let name = CString::new(user.as_str())?;
    unsafe {
        let pwd = libc::getpwnam(name.as_ptr());
        ensure!(!pwd.is_null(), "Unknown service user");
        let (uid, gid) = ((*pwd).pw_uid, (*pwd).pw_gid);
        let home = CStr::from_ptr((*pwd).pw_dir).to_string_lossy().into_owned();
        if libc::geteuid() == uid {
            return Ok(());
        }
        ensure!(
            libc::geteuid() == 0,
            "Run as the configured service user or root"
        );
        // This runs before the supervisor starts any threads. Match runuser/su's
        // account environment so application tools do not use root's home directory.
        std::env::set_var("HOME", home);
        std::env::set_var("USER", &user);
        std::env::set_var("LOGNAME", &user);
        ensure!(
            libc::initgroups(name.as_ptr(), gid as _) == 0
                && libc::setgid(gid) == 0
                && libc::setuid(uid) == 0,
            "Unable to switch to service user: {}",
            std::io::Error::last_os_error()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn service_arguments_are_escaped() {
        assert_eq!(
            unit_arg("/opt/Java App/$test%/a\"b").unwrap(),
            "\"/opt/Java App/$$test%%/a\\\"b\""
        );
        assert!(unit_value("name\nExecStart=evil").is_err());
        assert_eq!(xml("A&B <C>"), "A&amp;B &lt;C&gt;");
        assert_eq!(shell_quote("java'app"), "'java'\\''app'");
    }
}
