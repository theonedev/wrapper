use crate::{
    child::ManagedChild,
    config::{Config, unquote},
};
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    io::Read,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub fn command(
    config: &Config,
    arguments: &[String],
    port: u16,
    token: &str,
    mut cancelled: impl FnMut() -> Result<bool>,
) -> Result<Option<Command>> {
    let executable = unquote(config.get("wrapper.java.command", "java"));
    let mut probe = Command::new(&executable);
    probe
        .current_dir(config.working_dir())
        .envs(&config.environment)
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_console(&mut probe);
    let timeout = config.number("wrapper.startup.timeout", 30)?;
    let mut probe = ManagedChild::spawn(&mut probe).with_context(|| {
        format!("Launching {executable}; set wrapper.java.command to a Java 17+ executable")
    })?;
    let stdout = read_output(probe.child.stdout.take().unwrap());
    let stderr = read_output(probe.child.stderr.take().unwrap());
    let started = Instant::now();
    let status = (|| -> Result<_> {
        loop {
            if cancelled()? {
                return Ok(None);
            }
            let status = probe.try_wait()?;
            if status.is_some() {
                probe.kill();
            }
            if status.is_some() && stdout.is_finished() && stderr.is_finished() {
                return Ok(status);
            }
            ensure!(
                timeout == 0 || started.elapsed() < Duration::from_secs(timeout),
                "Java version check timed out"
            );
            thread::sleep(Duration::from_millis(25));
        }
    })();
    probe.kill();
    // An escaped helper may keep the pipes open. Join only after they drain;
    // timeout/cancellation must still let the wrapper exit.
    let Some(status) = status? else {
        return Ok(None);
    };
    let stdout = stdout.join().unwrap()?;
    let stderr = stderr.join().unwrap()?;
    let version_text = format!(
        "{}{}",
        String::from_utf8_lossy(&stdout),
        String::from_utf8_lossy(&stderr)
    );
    ensure!(
        status.success(),
        "Java version check failed ({status}):\n{version_text}"
    );
    let major = java_major(&version_text)
        .with_context(|| format!("Unable to read Java version:\n{version_text}"))?;
    ensure!(
        major >= config.number("wrapper.java.version.min", 17)?,
        "Java {major} does not meet wrapper.java.version.min"
    );

    let mut cmd = Command::new(executable);
    cmd.current_dir(config.working_dir())
        .envs(&config.environment);
    hide_console(&mut cmd);
    cmd.args(config.numbered("wrapper.java.additional."));
    for (property, flag) in [
        ("wrapper.java.initmemory", "-Xms"),
        ("wrapper.java.maxmemory", "-Xmx"),
    ] {
        if config.number(property, 0)? > 0 {
            cmd.arg(format!("{flag}{}m", config.number(property, 0)?));
        }
    }
    let libraries = config.numbered("wrapper.java.library.path.");
    if !libraries.is_empty() {
        let mut paths: Vec<_> = libraries
            .into_iter()
            .map(std::path::PathBuf::from)
            .collect();
        if config.boolean("wrapper.java.library.path.append_system_path", false)
            && let Some(path) = config.environment.get("PATH")
        {
            paths.extend(std::env::split_paths(path));
        }
        cmd.arg(format!(
            "-Djava.library.path={}",
            std::env::join_paths(paths)?.to_string_lossy()
        ));
    }
    let mut classpath = vec![config.boot.join("wrapper.jar")];
    ensure!(classpath[0].is_file(), "Missing {}", classpath[0].display());
    for pattern in config.numbered("wrapper.java.classpath.") {
        let path = config.working_dir().join(pattern);
        let filename = path.file_name().unwrap_or_default().to_string_lossy();
        if filename.contains('*') || filename.contains('?') {
            if let Ok(entries) = fs::read_dir(path.parent().unwrap_or(Path::new("."))) {
                let mut matches = Vec::new();
                for entry in entries {
                    let entry = entry?;
                    if wildcard(&filename, &entry.file_name().to_string_lossy()) {
                        matches.push(entry.path());
                    }
                }
                matches.sort();
                classpath.extend(matches);
            }
        } else {
            classpath.push(path);
        }
    }
    cmd.arg("-cp").arg(std::env::join_paths(classpath)?);
    // Launch any Java main class through the lifecycle bridge; no JSW JNI dependency.
    cmd.env("WRAPPER_PORT", port.to_string())
        .env("WRAPPER_TOKEN", token)
        .env(
            "WRAPPER_SHUTDOWN_TIMEOUT",
            config.number("wrapper.shutdown.timeout", 30)?.to_string(),
        );
    cmd.arg("io.onedev.wrapper.WrapperSimpleApp");
    let main = config.get(
        "wrapper.java.mainclass",
        "io.onedev.wrapper.WrapperSimpleApp",
    );
    if main != "io.onedev.wrapper.WrapperSimpleApp" {
        cmd.arg(main);
    }
    cmd.args(config.numbered("wrapper.app.parameter."))
        .args(arguments);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    Ok(Some(cmd))
}

fn read_output(
    mut input: impl Read + Send + 'static,
) -> thread::JoinHandle<std::io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        input.read_to_end(&mut bytes)?;
        Ok(bytes)
    })
}

fn hide_console(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = command;
}

fn java_major(output: &str) -> Option<u64> {
    let version = output
        .lines()
        .find_map(|line| line.split_once("version \"").map(|(_, version)| version))?;
    let version = version.strip_prefix("1.").unwrap_or(version);
    version
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

fn wildcard(pattern: &str, value: &str) -> bool {
    let (p, v) = (pattern.as_bytes(), value.as_bytes());
    let (mut i, mut j, mut star, mut retry) = (0, 0, None, 0);
    while j < v.len() {
        if i < p.len() && (p[i] == b'?' || p[i] == v[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == b'*' {
            star = Some(i);
            i += 1;
            retry = j;
        } else if let Some(s) = star {
            i = s + 1;
            retry += 1;
            j = retry;
        } else {
            return false;
        }
    }
    while i < p.len() && p[i] == b'*' {
        i += 1;
    }
    i == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn java_versions() {
        assert_eq!(
            java_major("openjdk version \"17.0.14\" 2025-01-21"),
            Some(17)
        );
        assert_eq!(java_major("java version \"1.8.0_292\""), Some(8));
        assert_eq!(
            java_major("Picked up JAVA_TOOL_OPTIONS: -Dvalue=\"text\"\nopenjdk version \"21.0.1\""),
            Some(21)
        );
    }
    #[test]
    fn classpath_patterns() {
        assert!(wildcard("*.jar", "java app.jar"));
        assert!(!wildcard("*.jar", "x.jar.bak"));
        assert!(wildcard("lib-?-*.jar", "lib-a-one.jar"));
    }
}
