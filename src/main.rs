mod child;
mod config;
mod java;
mod logging;
mod service;
#[cfg(unix)]
mod startup;
mod supervisor;

use anyhow::{Context, Result, bail, ensure};
use config::Config;
use std::{collections::HashMap, env, path::PathBuf, process};

const SERVICE_COMMANDS: &str = if cfg!(unix) {
    "console|start|stop|restart|status|install|remove|dump"
} else {
    "console|start|stop|restart|status|install|remove"
};

#[derive(Clone, Debug)]
pub struct Options {
    pub command: String,
    pub config: PathBuf,
    pub overrides: Vec<String>,
    pub arguments: Vec<String>,
    pub internal: bool,
}

impl Options {
    fn parse(mut args: Vec<String>) -> Result<Self> {
        ensure!(
            !args.is_empty(),
            "Usage: wrapper [{SERVICE_COMMANDS}] conf/wrapper.conf [property=value ...] [-- application arguments]"
        );
        let mut options = Self {
            command: "console".into(),
            config: PathBuf::new(),
            overrides: vec![],
            arguments: vec![],
            internal: false,
        };
        if args[0] == "--script" || args[0] == "--command" {
            let one_shot = args[0] == "--command";
            ensure!(args.len() >= 3, "Missing script name or configuration file");
            let script = args.remove(1);
            args.remove(0);
            options.config = args.remove(0).into();
            if !one_shot {
                // Installed Unix services retain the launcher's .shconf setup,
                // but supply their own config, overrides and application args.
                if args.first().is_some_and(|arg| arg == "--service") {
                    let mut service = Self::parse(args[1..].to_vec())?;
                    service.internal = true;
                    return Ok(service);
                }
                ensure!(!args.is_empty(), "Usage: {script} {SERVICE_COMMANDS}");
                options.command = args.remove(0);
                if args.first().is_some_and(|s| s == "sysd") {
                    args.remove(0);
                    options.internal = true;
                }
                if options.command == "launchdinternal" {
                    options.command = "console".into();
                    options.internal = true;
                }
            } else {
                options
                    .overrides
                    .push("wrapper.logfile.loglevel=NONE".into());
                options
                    .overrides
                    .push(format!("wrapper.name=command-{script}"));
                options
                    .overrides
                    .push("wrapper.restart.disabled=TRUE".into());
                options.arguments.push(script);
            }
            // Script arguments are always application arguments, never evaluated as shell code.
            if args.first().is_some_and(|s| s == "--") {
                args.remove(0);
            }
            options.arguments.extend(args);
        } else {
            let command = match args[0].as_str() {
                "-c" => Some("console"),
                "-s" => Some("service"),
                "-t" => Some("start"),
                "-p" => Some("stop"),
                "-i" => Some("install"),
                "-r" => Some("remove"),
                "-q" => Some("status"),
                "-it" => Some("installstart"),
                "console" | "daemon" | "service" | "start" | "stop" | "restart" | "status"
                | "install" | "remove" | "installstart" | "condrestart" | "dump" | "check"
                | "render-service" => Some(args[0].as_str()),
                _ => None,
            };
            if let Some(command) = command {
                options.command = command.to_owned();
                args.remove(0);
            }
            ensure!(!args.is_empty(), "Missing configuration file");
            options.config = args.remove(0).into();
            let mut application = false;
            for arg in args {
                if application {
                    options.arguments.push(arg);
                } else if arg == "--" {
                    application = true;
                } else {
                    ensure!(
                        arg.contains('='),
                        "Expected property=value or -- before {arg}"
                    );
                    options.overrides.push(arg);
                }
            }
        }
        options.config = std::fs::canonicalize(&options.config)
            .with_context(|| format!("Reading {}", options.config.display()))?;
        Ok(options)
    }

    pub fn load(&self) -> Result<Config> {
        self.load_with_environment(env::vars().collect())
    }
    pub fn load_with_environment(&self, environment: HashMap<String, String>) -> Result<Config> {
        Config::load(
            &self.config,
            env::current_exe()?
                .parent()
                .context("Executable directory missing")?,
            &self.overrides,
            environment,
        )
    }
    pub fn service_args(&self, command: &str) -> Vec<String> {
        let mut options = self.clone();
        options.overrides.retain(|property| {
            !property.split_once('=').is_some_and(|(key, _)| {
                matches!(
                    key.trim(),
                    "wrapper.ntservice.account" | "wrapper.ntservice.password"
                )
            })
        });
        options.child_args(command)
    }
    pub fn child_args(&self, command: &str) -> Vec<String> {
        let mut args = vec![
            command.to_owned(),
            self.config.to_string_lossy().into_owned(),
        ];
        args.extend(self.overrides.clone());
        args.push("--".into());
        args.extend(self.arguments.clone());
        args
    }
}

fn run() -> Result<i32> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args == ["--version"] {
        println!("ServiceWrapper {}", env!("CARGO_PKG_VERSION"));
        return Ok(0);
    }
    let options = Options::parse(args)?;
    let config = options.load()?;
    match options.command.as_str() {
        "console" | "daemon" => supervisor::run(&options, |_| Ok(())),
        "service" => service::dispatch(&options, &config),
        "check" => {
            java::command(&config, &options.arguments, 0, "", || Ok(false))?;
            println!("Configuration and Java executable OK");
            Ok(0)
        }
        "render-service" => {
            print!("{}", service::render(&options, &config)?);
            Ok(0)
        }
        "start" | "stop" | "restart" | "status" | "install" | "remove" | "installstart"
        | "condrestart" | "dump" => service::manage(&options, &config),
        command => bail!("Unknown command: {command}"),
    }
}

fn main() {
    let code = match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("ServiceWrapper: {error:#}");
            1
        }
    };
    process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_service_arguments_exclude_credentials() {
        let options = Options {
            command: "install".into(),
            config: "custom.conf".into(),
            overrides: vec![
                "wrapper.ntservice.account=service-user".into(),
                "wrapper.ntservice.password=first-secret".into(),
                " wrapper.ntservice.password = second-secret".into(),
                "wrapper.java.additional.1=-Xmx1g".into(),
            ],
            arguments: vec![
                "two words".into(),
                "wrapper.ntservice.password=application-data".into(),
            ],
            internal: false,
        };
        assert_eq!(
            options.service_args("-s"),
            vec![
                "-s",
                "custom.conf",
                "wrapper.java.additional.1=-Xmx1g",
                "--",
                "two words",
                "wrapper.ntservice.password=application-data",
            ]
        );
        assert!(
            options
                .child_args("daemon")
                .contains(&"wrapper.ntservice.password=first-secret".into())
        );
    }
}
