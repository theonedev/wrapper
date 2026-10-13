//! A bounded startup result, independent of log destinations and rotation.
#[cfg(unix)]
use anyhow::Context;
use anyhow::{Result, bail, ensure};
use std::{
    io::Write,
    sync::{Arc, Mutex},
};

const LIMIT: usize = 64 * 1024;

#[derive(Clone)]
pub struct Capture(Arc<Mutex<Report>>);

struct Report {
    sink: Sink,
    output: Vec<u8>,
    finished: bool,
}

enum Sink {
    #[cfg(unix)]
    File(std::fs::File, String),
    #[cfg(any(windows, test))]
    Socket(std::net::TcpStream, String),
}

impl Sink {
    fn send(&mut self, state: &str, output: &[u8]) -> Result<()> {
        match self {
            #[cfg(unix)]
            Self::File(file, id) => {
                use fs2::FileExt;
                use std::io::{Seek, SeekFrom};
                file.lock_exclusive()?;
                let result = (|| -> Result<()> {
                    file.seek(SeekFrom::Start(0))?;
                    file.set_len(0)?;
                    writeln!(file, "{id}\n{state}\n{}", output.len())?;
                    file.write_all(output)?;
                    file.flush()?;
                    Ok(())
                })();
                FileExt::unlock(file)?;
                result
            }
            #[cfg(any(windows, test))]
            Self::Socket(stream, id) => {
                // Closing/interruption of the start command must not stop the service.
                let _ = (|| -> std::io::Result<()> {
                    writeln!(stream, "{id}\n{state}\n{}", output.len())?;
                    stream.write_all(output)
                })();
                Ok(())
            }
        }
    }
}

impl Capture {
    fn new(mut sink: Sink) -> Result<Self> {
        sink.send("STARTING", &[])?;
        Ok(Self(Arc::new(Mutex::new(Report {
            sink,
            output: Vec::new(),
            finished: false,
        }))))
    }

    #[cfg(unix)]
    pub fn file(config: &crate::config::Config, id: &str) -> Result<Self> {
        use std::os::unix::fs::PermissionsExt;
        let file = crate::supervisor::secure_file(&path(config))?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        Self::new(Sink::File(file, format!("{}:{id}", std::process::id())))
    }

    #[cfg(any(windows, test))]
    pub fn connect(port: u16, token: &str) -> Result<Self> {
        use std::{net::TcpStream, time::Duration};
        let stream =
            TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_secs(5))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        Self::new(Sink::Socket(stream, token.to_owned()))
    }

    pub fn record(&self, bytes: &[u8]) {
        let mut report = self.0.lock().unwrap();
        if !report.finished {
            report.output.extend_from_slice(bytes);
            let excess = report.output.len().saturating_sub(LIMIT);
            report.output.drain(..excess);
        }
    }

    pub fn ready(&self) -> Result<()> {
        let mut report = self.0.lock().unwrap();
        if !report.finished {
            report.sink.send("READY", &[])?;
            report.finished = true;
            report.output.clear();
        }
        Ok(())
    }

    pub fn finish(&self) -> Result<()> {
        let mut report = self.0.lock().unwrap();
        if !report.finished {
            let output = std::mem::take(&mut report.output);
            let output = if output.is_empty() {
                b"JVM exited before completing startup".as_slice()
            } else {
                &output
            };
            report.sink.send("FAILED", output)?;
            report.finished = true;
        }
        Ok(())
    }
}

struct Message<'a> {
    id: &'a str,
    state: &'a str,
    output: &'a [u8],
    #[cfg(any(windows, test))]
    length: usize,
}

fn message(bytes: &[u8]) -> Result<Option<Message<'_>>> {
    let mut lines = bytes.splitn(4, |b| *b == b'\n');
    let Some((id, state, length, output)) = lines
        .next()
        .zip(lines.next())
        .zip(lines.next())
        .zip(lines.next())
        .map(|(((a, b), c), d)| (a, b, c, d))
    else {
        return Ok(None);
    };
    let length: usize = std::str::from_utf8(length)?.parse()?;
    ensure!(length <= LIMIT, "Startup diagnostics exceed size limit");
    if output.len() < length {
        return Ok(None);
    }
    Ok(Some(Message {
        id: std::str::from_utf8(id)?,
        state: std::str::from_utf8(state)?,
        output: &output[..length],
        #[cfg(any(windows, test))]
        length: bytes.len() - output.len() + length,
    }))
}

fn result(message: &Message<'_>) -> Result<bool> {
    match message.state {
        "READY" => Ok(true),
        "STARTING" => Ok(false),
        "FAILED" => bail!(
            "Service failed to start:\n{}",
            String::from_utf8_lossy(message.output).trim_end()
        ),
        state => bail!("Invalid startup state: {state}"),
    }
}

#[cfg(unix)]
pub fn path(config: &crate::config::Config) -> std::path::PathBuf {
    let mut path = config.pid_file().into_os_string();
    path.push(".startup");
    path.into()
}

#[cfg(unix)]
pub struct FileWait {
    path: std::path::PathBuf,
    previous: Option<String>,
    pid: Option<u32>,
}

#[cfg(unix)]
impl FileWait {
    pub fn new(path: std::path::PathBuf, pid: Option<u32>) -> Result<Self> {
        let mut wait = Self {
            path,
            previous: None,
            pid,
        };
        if pid.is_none() {
            let bytes = wait.read()?;
            wait.previous = message(&bytes)?.map(|m| m.id.to_owned());
        }
        Ok(wait)
    }

    fn read(&self) -> Result<Vec<u8>> {
        use fs2::FileExt;
        use std::{fs::OpenOptions, io::Read, os::unix::fs::OpenOptionsExt};
        let file = match OpenOptions::new()
            .read(true)
            // Reject FIFOs without blocking in open before the regular-file check.
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&self.path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error).context("Reading service startup result"),
        };
        ensure!(
            file.metadata()?.is_file(),
            "Expected regular startup result file"
        );
        match FileExt::try_lock_shared(&file) {
            Ok(()) => (),
            Err(error) if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() => {
                return Ok(Vec::new());
            }
            Err(error) => return Err(error).context("Locking service startup result"),
        }
        let mut bytes = Vec::new();
        (&file).take((LIMIT + 256) as u64).read_to_end(&mut bytes)?;
        FileExt::unlock(&file)?;
        Ok(bytes)
    }

    pub fn poll(&self) -> Result<bool> {
        let bytes = self.read()?;
        if let Some(message) = message(&bytes)?
            && self.previous.as_deref() != Some(message.id)
            && self
                .pid
                .is_none_or(|pid| message.id.starts_with(&format!("{pid}:")))
        {
            return result(&message);
        }
        Ok(false)
    }
}

// SCM passes these one-time connection arguments to the service process. This
// avoids assuming that the service account shares the caller's environment or paths.
#[cfg(any(windows, test))]
pub struct SocketWait {
    listener: std::net::TcpListener,
    token: String,
    pending: Vec<(std::net::TcpStream, Vec<u8>, std::time::Instant, bool)>,
}

#[cfg(any(windows, test))]
impl SocketWait {
    pub fn new() -> Result<Self> {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let mut random = [0u8; 32];
        getrandom::fill(&mut random)
            .map_err(|e| anyhow::anyhow!("Random number generation failed: {e}"))?;
        Ok(Self {
            listener,
            token: random.iter().map(|b| format!("{b:02x}")).collect(),
            pending: Vec::new(),
        })
    }

    pub fn arguments(&self) -> Result<[String; 2]> {
        Ok([
            self.listener.local_addr()?.port().to_string(),
            self.token.clone(),
        ])
    }

    #[cfg(windows)]
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn poll(&mut self) -> Result<bool> {
        use std::{
            io::Read,
            time::{Duration, Instant},
        };
        while self.pending.len() < 16 {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(true)?;
                    self.pending
                        .push((stream, Vec::new(), Instant::now(), false));
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error.into()),
            }
        }
        let mut index = 0;
        while index < self.pending.len() {
            let (stream, bytes, accepted, authenticated) = &mut self.pending[index];
            let mut buffer = [0u8; 8192];
            let mut remove = false;
            loop {
                match stream.read(&mut buffer) {
                    Ok(0) => {
                        remove = true;
                        break;
                    }
                    Ok(n) => {
                        bytes.extend_from_slice(&buffer[..n]);
                        if bytes.len() > LIMIT + 256 {
                            remove = true;
                            break;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(_) => {
                        remove = true;
                        break;
                    }
                }
            }
            loop {
                let message = match message(bytes) {
                    Ok(Some(message)) => message,
                    Ok(None) => break,
                    Err(_) => {
                        remove = true;
                        break;
                    }
                };
                if message.id != self.token {
                    remove = true;
                    break;
                }
                *authenticated = true;
                if result(&message)? {
                    return Ok(true);
                }
                let length = message.length;
                bytes.drain(..length);
            }
            if remove
                || bytes.len() > LIMIT + 256
                || (!*authenticated && accepted.elapsed() > Duration::from_secs(2))
            {
                self.pending.swap_remove(index);
            } else {
                index += 1;
            }
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait(receiver: &mut SocketWait) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if receiver.poll()? {
                return Ok(());
            }
            ensure!(
                Instant::now() < deadline,
                "Startup result was not delivered"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn socket_reports_bounded_diagnostics_without_a_log_file() {
        let mut receiver = SocketWait::new().unwrap();
        let [port, token] = receiver.arguments().unwrap();
        let capture = Capture::connect(port.parse().unwrap(), &token).unwrap();
        assert!(!receiver.poll().unwrap());
        capture.record(&vec![b'x'; LIMIT * 2]);
        capture.record(b"\nInvalid maximum heap size\n");
        capture.finish().unwrap();
        let error = wait(&mut receiver).unwrap_err().to_string();
        assert!(error.ends_with("Invalid maximum heap size"));
        assert!(error.len() < LIMIT + 100);
    }

    #[test]
    fn socket_ignores_other_callers_and_errors_after_handshake() {
        let mut receiver = SocketWait::new().unwrap();
        let [port, token] = receiver.arguments().unwrap();
        let stranger = Capture::connect(port.parse().unwrap(), "wrong-token").unwrap();
        stranger.ready().unwrap();
        assert!(!receiver.poll().unwrap());
        let capture = Capture::connect(port.parse().unwrap(), &token).unwrap();
        capture.ready().unwrap();
        capture.record(b"Application class loading failed");
        capture.finish().unwrap();
        wait(&mut receiver).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn file_result_is_fresh_and_survives_process_exit() {
        let directory = std::env::temp_dir().join(format!(
            "startup-result-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("result");
        let file = crate::supervisor::secure_file(&path).unwrap();
        let first = Capture::new(Sink::File(file.try_clone().unwrap(), "old".into())).unwrap();
        first.ready().unwrap();
        let wait = FileWait::new(path.clone(), None).unwrap();
        assert!(!wait.poll().unwrap());
        let second = Capture::new(Sink::File(file, "new".into())).unwrap();
        second.record(b"Java command not found");
        second.finish().unwrap();
        drop(second);
        assert!(
            wait.poll()
                .unwrap_err()
                .to_string()
                .contains("Java command not found")
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
}
