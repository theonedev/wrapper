use crate::config::Config;
use anyhow::{Context, Result};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::PathBuf,
    sync::mpsc::{self, SyncSender},
    thread::{self, JoinHandle},
};

pub struct Log {
    sender: SyncSender<Vec<u8>>,
    worker: JoinHandle<()>,
}
impl Log {
    pub fn new(config: &Config, console: bool) -> Result<Self> {
        let mut file = None;
        let path = config.path("wrapper.logfile", "../logs/console.log");
        if !config
            .get("wrapper.logfile.loglevel", "INFO")
            .eq_ignore_ascii_case("NONE")
        {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            file = Some(
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .with_context(|| format!("Opening {}", path.display()))?,
            );
        }
        let console = console
            && !config
                .get("wrapper.console.loglevel", "INFO")
                .eq_ignore_ascii_case("NONE");
        let max = size(config.get("wrapper.logfile.maxsize", "250k"))?;
        let count = config.number("wrapper.logfile.maxfiles", 3)?;
        let (sender, receiver) = mpsc::sync_channel::<Vec<u8>>(128);
        let worker = thread::spawn(move || {
            let mut length = file
                .as_ref()
                .and_then(|f| f.metadata().ok())
                .map(|m| m.len())
                .unwrap_or(0);
            for bytes in receiver {
                if console {
                    let _ = std::io::stdout().write_all(&bytes);
                    let _ = std::io::stdout().flush();
                }
                if file.is_some() {
                    let result = (|| -> Result<()> {
                        if max > 0 && length > 0 && length + bytes.len() as u64 > max {
                            // Close before rename, including on Windows.
                            file.take();
                            rotate(&path, count)?;
                            file = Some(OpenOptions::new().create(true).append(true).open(&path)?);
                            length = 0;
                        }
                        let output: &mut File = file.as_mut().unwrap();
                        output.write_all(&bytes)?;
                        output.flush()?;
                        length += bytes.len() as u64;
                        Ok(())
                    })();
                    if let Err(error) = result {
                        eprintln!("ServiceWrapper: log write failed: {error:#}");
                        file = None;
                    }
                }
            }
        });
        Ok(Self { sender, worker })
    }
    pub fn message(&self, text: &str) {
        let _ = self
            .sender
            .send(format!("ServiceWrapper: {text}\n").into_bytes());
    }
    pub fn pipe(&self, mut input: impl Read + Send + 'static) -> JoinHandle<()> {
        let sender = self.sender.clone();
        thread::spawn(move || {
            let mut bytes = [0; 8192];
            loop {
                match input.read(&mut bytes) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if sender.send(bytes[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        })
    }
    pub fn close(self) {
        drop(self.sender);
        let _ = self.worker.join();
    }
}
fn size(value: &str) -> Result<u64> {
    let value = value.to_ascii_lowercase();
    let (digits, multiplier) = if let Some(n) = value.strip_suffix('k') {
        (n, 1024)
    } else if let Some(n) = value.strip_suffix('m') {
        (n, 1024 * 1024)
    } else {
        (value.as_str(), 1)
    };
    digits
        .parse::<u64>()?
        .checked_mul(multiplier)
        .context("Log size overflow")
}
fn rotate(path: &PathBuf, count: u64) -> Result<()> {
    let rolled = |n| PathBuf::from(format!("{}.{n}", path.display()));
    let count = if count == 0 {
        let mut n = 1;
        while rolled(n).exists() {
            n += 1;
        }
        n
    } else {
        count
    };
    if rolled(count).exists() {
        fs::remove_file(rolled(count))?;
    }
    for n in (1..count).rev() {
        if rolled(n).exists() {
            fs::rename(rolled(n), rolled(n + 1))?;
        }
    }
    fs::rename(path, rolled(1))?;
    Ok(())
}
