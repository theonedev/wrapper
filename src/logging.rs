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
    sender: SyncSender<Entry>,
    worker: JoinHandle<()>,
    pub startup: Option<crate::startup::Capture>,
}

enum Entry {
    Output(Vec<u8>),
    Error(Vec<u8>),
    Flush(mpsc::Sender<()>),
}
impl Log {
    #[cfg(windows)]
    pub fn new(config: &Config, console: bool) -> Result<Self> {
        Self::with_startup(config, console, None)
    }
    pub fn with_startup(
        config: &Config,
        console: bool,
        startup: Option<crate::startup::Capture>,
    ) -> Result<Self> {
        let redirect = !console;
        let errors_to_stderr = console;
        let mut file = None;
        let path = config.path("wrapper.logfile", "../logs/console.log");
        if !config
            .get("wrapper.logfile.loglevel", "INFO")
            .eq_ignore_ascii_case("NONE")
        {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            file = Some(open_file(&path, redirect)?);
        }
        #[cfg(unix)]
        if redirect && file.is_none() {
            // A reload can disable logging after diagnostics were redirected to a file.
            redirect_output(&OpenOptions::new().write(true).open("/dev/null")?)?;
        }
        let console = console
            && !config
                .get("wrapper.console.loglevel", "INFO")
                .eq_ignore_ascii_case("NONE");
        let max = size(config.get("wrapper.logfile.maxsize", "250k"))?;
        let count = config.number("wrapper.logfile.maxfiles", 3)?;
        let (sender, receiver) = mpsc::sync_channel(128);
        let capture = startup.clone();
        let worker = thread::spawn(move || {
            let mut length = file
                .as_ref()
                .and_then(|f| f.metadata().ok())
                .map(|m| m.len())
                .unwrap_or(0);
            for entry in receiver {
                let (bytes, error) = match entry {
                    Entry::Output(bytes) => (bytes, false),
                    Entry::Error(bytes) => (bytes, true),
                    Entry::Flush(done) => {
                        let _ = done.send(());
                        continue;
                    }
                };
                if let Some(capture) = &capture {
                    capture.record(&bytes);
                }
                if error && errors_to_stderr {
                    let _ = std::io::stderr().write_all(&bytes);
                    let _ = std::io::stderr().flush();
                } else if !error && console {
                    let _ = std::io::stdout().write_all(&bytes);
                    let _ = std::io::stdout().flush();
                }
                if file.is_some() {
                    let result = (|| -> Result<()> {
                        if max > 0 && length > 0 && length + bytes.len() as u64 > max {
                            // Close before rename, including on Windows.
                            file.take();
                            rotate(&path, count)?;
                            file = Some(open_file(&path, redirect)?);
                            length = 0;
                        }
                        let output: &mut File = file.as_mut().unwrap();
                        output.write_all(&bytes)?;
                        output.flush()?;
                        length += bytes.len() as u64;
                        Ok(())
                    })();
                    if let Err(error) = result {
                        eprintln!("Wrapper: log write failed: {error:#}");
                        file = None;
                    }
                }
            }
        });
        Ok(Self {
            sender,
            worker,
            startup,
        })
    }
    pub fn message(&self, text: &str) {
        let _ = self
            .sender
            .send(Entry::Output(format!("Wrapper: {text}\n").into_bytes()));
    }
    #[cfg(windows)]
    pub fn reporter(&self) -> impl Fn(&str) + use<> {
        let sender = self.sender.clone();
        move |text| {
            let _ = sender.send(Entry::Output(format!("Wrapper: {text}\n").into_bytes()));
        }
    }
    pub fn error(&self, text: &str) {
        let _ = self
            .sender
            .send(Entry::Error(format!("Wrapper: {text}\n").into_bytes()));
    }
    pub fn pipe(&self, mut input: impl Read + Send + 'static) -> JoinHandle<()> {
        let sender = self.sender.clone();
        thread::spawn(move || {
            let mut bytes = [0; 8192];
            loop {
                match input.read(&mut bytes) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if sender.send(Entry::Output(bytes[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                }
            }
        })
    }
    pub fn flush(&self) {
        let (done, receiver) = mpsc::channel();
        if self.sender.send(Entry::Flush(done)).is_ok() {
            let _ = receiver.recv();
        }
    }
    pub fn close(self) {
        drop(self.sender);
        let _ = self.worker.join();
    }
}
fn open_file(path: &PathBuf, redirect: bool) -> Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("Opening {}", path.display()))?;
    #[cfg(unix)]
    if redirect {
        // Detached diagnostics must follow rotation and configuration reloads,
        // rather than continue writing to an old (eventually unlinked) inode.
        redirect_output(&file)?;
    }
    #[cfg(not(unix))]
    let _ = redirect;
    Ok(file)
}
#[cfg(unix)]
fn redirect_output(file: &File) -> Result<()> {
    use std::os::fd::AsRawFd;
    for target in [libc::STDOUT_FILENO, libc::STDERR_FILENO] {
        if unsafe { libc::dup2(file.as_raw_fd(), target) } < 0 {
            return Err(std::io::Error::last_os_error()).context("Redirecting daemon output");
        }
    }
    Ok(())
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
