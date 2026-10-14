use anyhow::{Context, Result, bail, ensure};
use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct Config {
    pub file: PathBuf,
    pub boot: PathBuf,
    pub values: BTreeMap<String, String>,
    pub environment: HashMap<String, String>,
}

impl Config {
    pub fn load(
        file: &Path,
        boot: &Path,
        overrides: &[String],
        environment: HashMap<String, String>,
    ) -> Result<Self> {
        let mut config = Self {
            file: fs::canonicalize(file).with_context(|| format!("Reading {}", file.display()))?,
            boot: boot.to_path_buf(),
            values: BTreeMap::new(),
            environment: environment
                .into_iter()
                .map(|(name, value)| (environment_key(&name).into_owned(), value))
                .collect(),
        };
        config.read(&config.file.clone(), &mut Vec::new())?;
        for property in overrides {
            config.property(property)?;
        }
        // Resolve after reading defaults, as the shipped config uses defaults below their references.
        let environment = config.environment.clone();
        for value in config.environment.values_mut() {
            *value = expand(value, &environment)?;
        }
        for value in config.values.values_mut() {
            *value = expand(value, &config.environment)?;
        }
        let name = config.get("wrapper.name", "java-service");
        #[cfg(unix)]
        ensure!(
            !name.is_empty()
                && name.len() <= 128
                && name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c)),
            "wrapper.name must contain only letters, digits, _, - or ."
        );
        #[cfg(windows)]
        ensure!(
            !name.is_empty()
                && name.len() <= 128
                && !name
                    .chars()
                    .any(|c| c.is_control() || "\\/:*?\"<>|".contains(c)),
            "wrapper.name must be a valid Windows service and file name"
        );
        #[cfg(target_os = "macos")]
        ensure!(
            !config.launchd_label().is_empty()
                && config.launchd_label().len() <= 128
                && config
                    .launchd_label()
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c)),
            "wrapper.launchd.label must contain 1 to 128 letters, digits, _, - or ."
        );
        // These require JSW's proprietary integrations. Never silently accept a setting we cannot implement.
        for key in config.values.keys() {
            #[cfg(not(windows))]
            ensure!(
                !key.starts_with("wrapper.share."),
                "{key} is only supported on Windows"
            );
            if key.starts_with("wrapper.filter.")
                || key.starts_with("wrapper.event.")
                || key.starts_with("wrapper.timer.")
            {
                bail!(
                    "Unsupported JSW property: {key}. Configure this integration with the operating system service manager."
                );
            }
        }
        #[cfg(windows)]
        crate::shares::parse(&config)?;
        ensure!(
            !config.boolean("wrapper.ntservice.interactive", false),
            "Interactive Windows services are not supported"
        );
        Ok(config)
    }

    fn read(&mut self, file: &Path, stack: &mut Vec<PathBuf>) -> Result<()> {
        let file = fs::canonicalize(file)?;
        ensure!(
            stack.len() < 32 && !stack.contains(&file),
            "Recursive configuration include: {}",
            file.display()
        );
        stack.push(file.clone());
        let source = fs::read_to_string(&file)
            .with_context(|| format!("Reading UTF-8 configuration {}", file.display()))?;
        for raw in source.trim_start_matches('\u{feff}').lines() {
            let line = raw.trim();
            if let Some(encoding) = line.strip_prefix("#encoding=") {
                ensure!(
                    encoding.eq_ignore_ascii_case("UTF-8") || encoding.eq_ignore_ascii_case("UTF8"),
                    "Only UTF-8 configuration is supported"
                );
            } else if line.starts_with("#include ") || line.starts_with("#include.required ") {
                let (directive, path) = line.split_once(' ').unwrap();
                let path = self
                    .boot
                    .join(unquote(&expand(path.trim(), &self.environment)?));
                if path.exists() {
                    self.read(&path, stack)?;
                } else {
                    ensure!(
                        directive != "#include.required",
                        "Required include missing: {}",
                        path.display()
                    );
                }
            } else if !line.is_empty() && !line.starts_with('#') {
                // Ignore non-property lines for compatibility with existing configuration files.
                if line.contains('=') {
                    self.property(&strip_comment(line))?;
                }
            }
        }
        stack.pop();
        Ok(())
    }

    fn property(&mut self, line: &str) -> Result<()> {
        let (key, value) = line.split_once('=').context("Expected property=value")?;
        let (key, value) = (key.trim(), value.trim());
        if let Some(name) = key.strip_prefix("set.default.") {
            let name = environment_key(name);
            if !self.environment.contains_key(name.as_ref()) {
                let value = expand(value, &self.environment)?;
                self.environment.insert(name.into_owned(), value);
            }
        } else if let Some(name) = key.strip_prefix("set.") {
            // Capture existing values before overwriting them, e.g. set.PATH=...:%PATH%.
            // References that are not defined yet are resolved after reading the file.
            let value = expand(value, &self.environment)?;
            self.environment
                .insert(environment_key(name).into_owned(), value);
        } else {
            self.values.insert(key.into(), value.into());
        }
        Ok(())
    }

    pub fn get<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.values.get(key).map(String::as_str).unwrap_or(default)
    }
    pub fn boolean(&self, key: &str, default: bool) -> bool {
        self.values
            .get(key)
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(default)
    }
    pub fn number(&self, key: &str, default: u64) -> Result<u64> {
        self.values
            .get(key)
            .map(|v| {
                v.parse()
                    .with_context(|| format!("Invalid integer for {key}"))
            })
            .unwrap_or(Ok(default))
    }
    pub fn numbered(&self, prefix: &str) -> Vec<String> {
        let mut entries: Vec<_> = self
            .values
            .iter()
            .filter_map(|(k, v)| {
                k.strip_prefix(prefix)?
                    .parse::<u32>()
                    .ok()
                    .map(|i| (i, unquote(v)))
            })
            .collect();
        entries.sort_by_key(|(i, _)| *i);
        entries
            .into_iter()
            .filter_map(|(_, v)| (!v.is_empty()).then_some(v))
            .collect()
    }
    pub fn name(&self) -> &str {
        self.get("wrapper.name", "java-service")
    }
    #[cfg(unix)]
    pub fn launchd_label(&self) -> &str {
        self.get("wrapper.launchd.label", self.name())
    }
    pub fn working_dir(&self) -> PathBuf {
        self.boot
            .join(unquote(self.get("wrapper.working.dir", ".")))
    }
    pub fn path(&self, key: &str, default: &str) -> PathBuf {
        self.working_dir().join(unquote(self.get(key, default)))
    }
    pub fn pid_file(&self) -> PathBuf {
        self.path("wrapper.pidfile", &format!("../bin/{}.pid", self.name()))
    }
    pub fn control_file(&self) -> PathBuf {
        let mut path = self.pid_file().into_os_string();
        path.push(".control");
        path.into()
    }
    pub fn user(&self) -> String {
        self.get(
            "wrapper.daemon.user",
            self.environment
                .get("RUN_AS_USER")
                .map(String::as_str)
                .unwrap_or(""),
        )
        .to_owned()
    }
}

fn environment_key(name: &str) -> Cow<'_, str> {
    // Keep one entry per Windows variable, including inherited names such as Path.
    #[cfg(windows)]
    return Cow::Owned(name.to_uppercase());
    #[cfg(not(windows))]
    Cow::Borrowed(name)
}

fn strip_comment(line: &str) -> String {
    let mut chars = line.chars().peekable();
    let mut value = String::new();
    while let Some(c) = chars.next() {
        if c == '#' {
            if chars.peek() == Some(&'#') {
                chars.next();
            } else {
                break;
            }
        }
        value.push(c);
    }
    value.trim().to_owned()
}

pub fn unquote(value: &str) -> String {
    // A numbered JVM option is one argument, even when it contains spaces. Do not use a shell
    // parser: it would consume Windows path separators or split property values.
    let value = value.trim();
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        value[1..value.len() - 1].to_owned()
    } else {
        value.replace('"', "")
    }
}

fn expand(source: &str, environment: &HashMap<String, String>) -> Result<String> {
    // A duplicating cycle can exhaust memory long before the iteration limit.
    const MAX_EXPANSION: usize = 1024 * 1024;
    let mut value = source.to_owned();
    for _ in 0..32 {
        ensure!(
            value.len() <= MAX_EXPANSION,
            "Environment expansion exceeds 1 MiB (possible cycle)"
        );
        let mut output = String::new();
        let mut rest = value.as_str();
        let mut expanded = false;
        while let Some(start) = rest.find('%') {
            output.push_str(&rest[..start]);
            let tail = &rest[start + 1..];
            if let Some(end) = tail.find('%') {
                let name = &tail[..end];
                if let Some(replacement) = environment.get(environment_key(name).as_ref()) {
                    ensure!(
                        output.len().saturating_add(replacement.len()) <= MAX_EXPANSION,
                        "Environment expansion exceeds 1 MiB (possible cycle)"
                    );
                    expanded = true;
                    output.push_str(replacement);
                } else {
                    output.push_str(&rest[start..start + end + 2]);
                }
                rest = &tail[end + 1..];
            } else {
                output.push_str(&rest[start..]);
                rest = "";
            }
        }
        output.push_str(rest);
        if output == value {
            ensure!(!expanded, "Cyclic environment expansion in configuration");
            return Ok(value);
        }
        value = output;
    }
    bail!("Cyclic environment expansion in configuration")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expansion_and_quoting_preserve_jvm_arguments() {
        let env = HashMap::from([
            ("MEMORY".into(), "50".into()),
            ("ALIAS".into(), "%MEMORY%".into()),
        ]);
        assert_eq!(
            expand("-XX:MaxRAMPercentage=%ALIAS%", &env).unwrap(),
            "-XX:MaxRAMPercentage=50"
        );
        assert_eq!(
            unquote(r#"-Dpath="C:\Program Files\Java""#),
            r"-Dpath=C:\Program Files\Java"
        );
        assert_eq!(strip_comment("value=abc##def # comment"), "value=abc#def");
    }
    #[test]
    fn cycles_fail() {
        assert!(
            expand(
                "%A%",
                &HashMap::from([("A".into(), "%B%".into()), ("B".into(), "%A%".into())])
            )
            .is_err()
        );
    }
}
