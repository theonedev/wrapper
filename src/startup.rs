//! Startup results from the real Unix service process, including its environment.
use crate::config::Config;
use anyhow::{Context, Result, ensure};
use std::{
    collections::hash_map::DefaultHasher,
    fs::{self, DirBuilder, OpenOptions},
    hash::{Hash, Hasher},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::PathBuf,
};

fn directory(uid: u32) -> PathBuf {
    // launchd and interactive shells have different TMPDIR values.
    PathBuf::from(format!("/tmp/servicewrapper-{uid}"))
}

fn validate_directory(uid: u32) -> Result<()> {
    let path = directory(uid);
    let metadata = fs::symlink_metadata(&path)?;
    ensure!(
        metadata.is_dir() && metadata.uid() == uid && metadata.mode() & 0o077 == 0,
        "Startup directory must be private and owned by service user {uid}: {}",
        path.display()
    );
    Ok(())
}

fn path(config: &Config, uid: u32) -> PathBuf {
    let mut hash = DefaultHasher::new();
    config.boot.hash(&mut hash);
    config.name().hash(&mut hash);
    directory(uid).join(format!("{:016x}.startup", hash.finish()))
}

pub struct Report {
    path: PathBuf,
    generation: String,
}

impl Report {
    pub fn new(config: &Config, generation: &str) -> Result<Self> {
        let uid = unsafe { libc::geteuid() };
        match DirBuilder::new().mode(0o700).create(directory(uid)) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error).context("Creating startup directory"),
        }
        validate_directory(uid)?;
        let report = Self {
            path: path(config, uid),
            generation: generation.into(),
        };
        report.write("STARTING", "")?;
        Ok(report)
    }

    pub fn write(&self, state: &str, message: &str) -> Result<()> {
        let temporary = self.path.with_extension(&self.generation);
        let result = (|| -> Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)?;
            writeln!(
                file,
                "{}\n{}\n{state}\n{message}",
                self.generation,
                std::process::id()
            )?;
            // Atomic replacement prevents readers from seeing a partial result.
            fs::rename(&temporary, &self.path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result.context("Writing service startup result")
    }
}

pub struct ResultRecord {
    pub generation: String,
    pub pid: u32,
    pub state: String,
    pub message: String,
}

pub fn read(config: &Config, uid: u32) -> Result<Option<ResultRecord>> {
    if !directory(uid).exists() {
        return Ok(None);
    }
    validate_directory(uid)?;
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path(config, uid))
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("Reading service startup result"),
    };
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.uid() == uid && metadata.mode() & 0o077 == 0,
        "Startup result must be private and owned by service user {uid}"
    );
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    let mut fields = text.splitn(4, '\n');
    Ok(Some(ResultRecord {
        generation: fields.next().context("Missing startup generation")?.into(),
        pid: fields.next().context("Missing startup PID")?.parse()?,
        state: fields.next().context("Missing startup state")?.into(),
        message: fields.next().unwrap_or("").trim_end().into(),
    }))
}
