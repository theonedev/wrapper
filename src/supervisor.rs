use crate::{Options, child::ManagedChild, config::Config, java, logging::Log};
use anyhow::{Context, Result, bail, ensure};
use fs2::FileExt;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, PartialEq)]
pub enum State {
    Starting,
    Running,
    Stopping,
}
pub static STOP: AtomicBool = AtomicBool::new(false);

pub(crate) fn secure_file(path: &PathBuf) -> Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .with_context(|| format!("Opening {}", path.display()))?;
    ensure!(
        file.metadata()?.is_file(),
        "Expected regular file: {}",
        path.display()
    );
    Ok(file)
}

struct Instance {
    lock: File,
    control: PathBuf,
}
impl Instance {
    fn acquire(config: &Config, port: u16, token: &str) -> Result<Self> {
        let path = config.pid_file();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut lock = secure_file(&path)?;
        lock.try_lock_exclusive()
            .context("This service instance is already running")?;
        lock.set_len(0)?;
        writeln!(lock, "{}", std::process::id())?;
        let instance = Self {
            lock,
            control: config.control_file(),
        };
        let mut file = secure_file(&instance.control)?;
        #[cfg(windows)]
        crate::service::protect_control_file(&instance.control)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        file.set_len(0)?;
        write!(file, "{port}\n{token}\n")?;
        file.sync_all()?;
        Ok(instance)
    }
}
impl Drop for Instance {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.control);
        // Keep the inode: unlinking a locked file permits two independent locks on one pathname.
        let _ = self.lock.set_len(0);
    }
}

pub fn request(config: &Config, command: &str) -> Result<Option<String>> {
    let lock = match OpenOptions::new()
        .read(true)
        .write(true)
        .open(config.pid_file())
    {
        Ok(lock) => lock,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error)
            if command == "status"
                && error.kind() == std::io::ErrorKind::PermissionDenied
                && fs::metadata(config.pid_file())
                    .is_ok_and(|metadata| metadata.is_file() && metadata.len() == 0) =>
        {
            // Shutdown retains an empty PID file, possibly owned by another account.
            // Its metadata is enough to report STOPPED without access to control files.
            return Ok(None);
        }
        Err(error) => {
            return Err(error).context("Run this command as the service account or administrator");
        }
    };
    for attempt in 0..20 {
        match lock.try_lock_exclusive() {
            Ok(()) => return Ok(None),
            Err(error) if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() => (),
            Err(error) => return Err(error.into()),
        }
        match exchange_control(config, command) {
            Ok(response) => return Ok(Some(response)),
            Err(error) => {
                let retry = error.downcast_ref::<std::io::Error>().is_none_or(|error| {
                    matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound
                            | std::io::ErrorKind::ConnectionRefused
                            | std::io::ErrorKind::ConnectionReset
                            | std::io::ErrorKind::UnexpectedEof
                            | std::io::ErrorKind::BrokenPipe
                    )
                });
                if !retry || attempt == 19 {
                    return Err(error);
                }
                // Startup publishes the control file after locking. Shutdown closes the
                // listener just before releasing the lock. Recheck rather than reporting
                // either short transition as a failed start/stop operation.
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
    unreachable!()
}

fn exchange_control(config: &Config, command: &str) -> Result<String> {
    let control = fs::read_to_string(config.control_file()).context("Service is starting, or control file is not readable; retry as the service account or administrator")?;
    let mut lines = control.lines();
    let port: u16 = lines.next().context("Missing control port")?.parse()?;
    let token = lines.next().context("Missing control token")?;
    let mut stream =
        TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_secs(2))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    writeln!(stream, "{token} {command}")?;
    read_line(&mut stream)
}

fn read_line(stream: &mut TcpStream) -> Result<String> {
    let mut bytes = Vec::new();
    let mut byte = [0];
    while bytes.len() < 512 {
        stream.read_exact(&mut byte)?;
        if byte[0] == b'\n' {
            return Ok(String::from_utf8(bytes)?);
        }
        bytes.push(byte[0]);
    }
    bail!("Control message too long")
}

struct Pending {
    stream: TcpStream,
    bytes: Vec<u8>,
    accepted: Instant,
}

// A JVM can connect before its writer classes finish initializing. Read handshakes
// incrementally, so a slow JVM or unauthenticated client never stalls supervision.
fn next_message(
    listener: &TcpListener,
    pending: &mut Vec<Pending>,
) -> Result<Option<(TcpStream, String)>> {
    while pending.len() < 32 {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(true)?;
                pending.push(Pending {
                    stream,
                    bytes: Vec::new(),
                    accepted: Instant::now(),
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) => return Err(error.into()),
        }
    }
    let mut index = 0;
    while index < pending.len() {
        let connection = &mut pending[index];
        let mut buffer = [0; 512];
        let remove = match connection.stream.read(&mut buffer) {
            Ok(0) => true,
            Ok(length) => {
                connection.bytes.extend_from_slice(&buffer[..length]);
                if let Some(end) = connection.bytes.iter().position(|b| *b == b'\n')
                    && end < 512
                {
                    let connection = pending.swap_remove(index);
                    if let Ok(message) = String::from_utf8(connection.bytes[..end].to_vec()) {
                        connection.stream.set_nonblocking(false)?;
                        connection
                            .stream
                            .set_write_timeout(Some(Duration::from_secs(2)))?;
                        return Ok(Some((connection.stream, message)));
                    }
                    continue;
                }
                connection.bytes.len() >= 512
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) =>
            {
                false
            }
            Err(_) => true,
        };
        if remove || connection.accepted.elapsed() >= Duration::from_secs(10) {
            pending.swap_remove(index);
        } else {
            index += 1;
        }
    }
    Ok(None)
}

#[cfg(unix)]
pub fn start(options: &Options, config: &Config) -> Result<i32> {
    use std::process::Command;

    // Open startup diagnostics as the service account: an inherited writable
    // descriptor retains its access even after the daemon drops privileges.
    crate::service::switch_user(config)?;
    // Match the daemon's paths and set.* overrides after the account switch.
    let config = options.load()?;
    if request(&config, "status")?.is_some() {
        println!("{} is already running", config.name());
        return Ok(0);
    }
    // Validate before spawning: an invalid wait must not leave a daemon behind.
    let timeout = config.number("wrapper.startup.wait.timeout", 120)?;
    let startup = crate::startup::FileWait::new(crate::startup::path(&config), None)?;
    let started = if options.internal {
        "Service started"
    } else {
        "Started"
    };
    println!(
        "{}",
        if options.internal {
            "Starting service..."
        } else {
            "Starting..."
        }
    );
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(options.child_args("daemon"))
        .stdin(std::process::Stdio::null());
    if config
        .get("wrapper.logfile.loglevel", "INFO")
        .eq_ignore_ascii_case("NONE")
    {
        command
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
    } else {
        // Preserve startup diagnostics even if Java cannot be launched.
        let diagnostic = config.path("wrapper.logfile", "../logs/console.log");
        if let Some(parent) = diagnostic.parent() {
            fs::create_dir_all(parent)?;
        }
        let output = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&diagnostic)
            .with_context(|| format!("Opening {}", diagnostic.display()))?;
        command.stdout(output.try_clone()?).stderr(output);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut child = command.spawn()?;
    let start = Instant::now();
    while timeout == 0 || start.elapsed() < Duration::from_secs(timeout) {
        if startup.poll()? {
            println!("{started}");
            return Ok(0);
        }
        if let Some(status) = child.try_wait()? {
            // The final result may have been written between polling and exit.
            if startup.poll()? {
                println!("{started}");
                return Ok(0);
            }
            bail!("{} failed to start ({status})", config.name());
        }
        thread::sleep(Duration::from_millis(100));
    }
    let _ = request(&config, "stop");
    bail!("Startup timed out; stop requested")
}

pub fn stop(config: &Config, installed: bool) -> Result<i32> {
    println!(
        "{}",
        if installed {
            "Stopping service..."
        } else {
            "Stopping..."
        }
    );
    if request(config, "stop")?.is_some() {
        // Zero has the JSW meaning: allow shutdown hooks to complete without a time limit.
        while request(config, "status")?.is_some() {
            thread::sleep(Duration::from_millis(100));
        }
    }
    println!(
        "{}",
        if installed {
            "Service stopped"
        } else {
            "Stopped"
        }
    );
    Ok(0)
}

pub fn run(
    options: &Options,
    state_changed: impl FnMut(State) -> Result<()>,
    startup: Option<crate::startup::Capture>,
) -> Result<i32> {
    #[cfg(unix)]
    crate::service::switch_user(&options.load()?)?;
    // Reload after an account switch to apply set.* overrides to the account's environment.
    let initial = options.load()?;
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    listener.set_nonblocking(true)?;
    let mut random = [0u8; 32];
    getrandom::fill(&mut random)
        .map_err(|e| anyhow::anyhow!("Random number generation failed: {e}"))?;
    let token: String = random.iter().map(|b| format!("{b:02x}")).collect();
    let _instance = Instance::acquire(&initial, listener.local_addr()?.port(), &token)?;
    #[cfg(unix)]
    let startup = if options.command == "daemon" || options.internal {
        Some(crate::startup::Capture::file(&initial, &token)?)
    } else {
        startup
    };
    let mut log = match Log::with_startup(
        &initial,
        options.command != "daemon" && options.command != "service",
        startup.clone(),
    ) {
        Ok(log) => log,
        Err(error) => {
            if let Some(startup) = &startup {
                startup.record(format!("{error:#}").as_bytes());
                startup.finish()?;
            }
            return Err(error);
        }
    };
    let code = match run_logged(options, initial, &mut log, listener, token, state_changed) {
        Ok(code) => code,
        Err(error) => {
            // Service managers may discard stderr. Use the active logger and drain
            // it before returning, including failures before the JVM is spawned.
            log.error(&format!("{error:#}"));
            1
        }
    };
    log.close();
    if let Some(startup) = startup {
        startup.finish()?;
    }
    Ok(code)
}

fn run_logged(
    options: &Options,
    initial: Config,
    log: &mut Log,
    listener: TcpListener,
    token: String,
    mut state_changed: impl FnMut(State) -> Result<()>,
) -> Result<i32> {
    let mut pending = Vec::new();
    let port = listener.local_addr()?.port();
    let signaled = Arc::new(AtomicBool::new(false));
    let handler = signaled.clone();
    // SCM services have no console. Their control handler sets STOP directly.
    if options.command != "service" {
        ctrlc::set_handler(move || {
            handler.store(true, Ordering::SeqCst);
        })?;
    }
    let mut config = initial.clone();
    loop {
        if signaled.load(Ordering::SeqCst) || STOP.load(Ordering::SeqCst) {
            return Ok(0);
        }
        state_changed(State::Starting)?;
        let command = java::command(&config, &options.arguments, port, &token, || {
            if signaled.load(Ordering::SeqCst) || STOP.load(Ordering::SeqCst) {
                return Ok(true);
            }
            if let Some((mut stream, message)) = next_message(&listener, &mut pending)? {
                if message == format!("{token} stop") {
                    let _ = stream.write_all(b"OK\n");
                    return Ok(true);
                }
                if message == format!("{token} status") {
                    let _ = stream.write_all(b"STARTING\n");
                }
            }
            Ok(false)
        })?;
        let Some(mut command) = command else {
            state_changed(State::Stopping)?;
            return Ok(0);
        };
        let mut jvm = ManagedChild::spawn(&mut command)?;
        let stdout = log.pipe(jvm.child.stdout.take().unwrap());
        let stderr = log.pipe(jvm.child.stderr.take().unwrap());
        let started = Instant::now();
        let mut ready = false;
        let mut bridge: Option<TcpStream> = None;
        let mut restart = false;
        let mut requested_exit = None;
        let mut stopping: Option<Instant> = None;
        let mut stop_reported = false;
        let mut forced = false;
        let exit;
        loop {
            if let Some(status) = jvm.child.try_wait()? {
                exit = status.code().unwrap_or(1);
                break;
            }
            if signaled.load(Ordering::SeqCst) || STOP.load(Ordering::SeqCst) {
                requested_exit = Some(0);
                restart = false;
            }
            if requested_exit.is_some() && !stop_reported {
                state_changed(State::Stopping)?;
                stop_reported = true;
            }
            if (requested_exit.is_some() || restart) && stopping.is_none() {
                stopping = Some(Instant::now());
                if let Some(stream) = &mut bridge {
                    let _ = stream.write_all(b"STOP\n");
                }
                #[cfg(unix)]
                if bridge.is_none() {
                    unsafe {
                        libc::kill(jvm.child.id() as i32, libc::SIGTERM);
                    }
                }
            }
            if let Some(stopping) = stopping {
                let timeout = config.number("wrapper.shutdown.timeout", 30)?;
                if timeout > 0 && stopping.elapsed() >= Duration::from_secs(timeout) {
                    log.message("Shutdown timeout expired; terminating JVM and its children");
                    jvm.kill();
                    forced = true;
                }
            } else {
                let timeout = config.number("wrapper.startup.timeout", 30)?;
                if !ready && timeout > 0 && started.elapsed() >= Duration::from_secs(timeout) {
                    log.message("JVM bridge startup timed out");
                    jvm.kill();
                    forced = true;
                }
            }
            if let Some((mut stream, message)) = next_message(&listener, &mut pending)?
                && let Some(command) = message.strip_prefix(&format!("{token} "))
            {
                match command {
                    "JVM" if bridge.is_none() => {
                        stream.write_all(b"OK\n")?;
                        // The bridge sends requests on separate connections. This channel is
                        // retained for graceful stop and parent-death detection.
                        if stopping.is_some() {
                            let _ = stream.write_all(b"STOP\n");
                        }
                        bridge = Some(stream);
                        ready = true;
                        if stopping.is_none() {
                            state_changed(State::Running)?;
                            if let Some(startup) = &log.startup {
                                startup.ready()?;
                            }
                        }
                    }
                    "status" => {
                        let _ = writeln!(
                            stream,
                            "{}",
                            if stopping.is_some() {
                                "STOPPING"
                            } else if ready {
                                "RUNNING"
                            } else {
                                "STARTING"
                            }
                        );
                    }
                    "stop" => {
                        requested_exit = Some(0);
                        restart = false;
                        let _ = stream.write_all(b"OK\n");
                    }
                    "restart" => {
                        if requested_exit.is_none() {
                            restart = true;
                        }
                        let _ = stream.write_all(b"OK\n");
                    }
                    "FAIL" | "SHUTDOWN" => {
                        // Bound hooks for main failures and JVM-initiated shutdown
                        // without overriding the actual exit code or restart policy.
                        stopping.get_or_insert_with(Instant::now);
                        let _ = stream.write_all(b"OK\n");
                    }
                    "dump" => {
                        #[cfg(unix)]
                        unsafe {
                            libc::kill(jvm.child.id() as i32, libc::SIGQUIT);
                        }
                        let _ = stream.write_all(b"OK\n");
                    }
                    command if command.starts_with("EXIT ") => {
                        if let Ok(code) = command[5..].parse::<i32>() {
                            // An operator's stop wins a race with an application restart.
                            if requested_exit.is_none() && !restart {
                                requested_exit = Some(code);
                            }
                            let _ = stream.write_all(b"OK\n");
                        }
                    }
                    _ => {
                        let _ = stream.write_all(b"ERROR\n");
                    }
                }
            }
            thread::sleep(Duration::from_millis(25));
        }
        drop(bridge);
        jvm.kill(); // Close inherited output pipes held by remaining JVM descendants.
        let _ = stdout.join();
        let _ = stderr.join();
        let stopped = signaled.load(Ordering::SeqCst) || STOP.load(Ordering::SeqCst);
        if stopped || requested_exit.is_some() {
            return Ok(if forced {
                1
            } else {
                requested_exit.unwrap_or(0)
            });
        }
        // An exit, crash or timeout stops the wrapper. Only an explicit JVM
        // restart request starts another invocation, with no retry limit.
        if !restart {
            return Ok(exit);
        }
        log.message("Restarting JVM (configuration will be reloaded)");
        let delay = Instant::now();
        while delay.elapsed() < Duration::from_secs(config.number("wrapper.restart.delay", 5)?) {
            if signaled.load(Ordering::SeqCst) || STOP.load(Ordering::SeqCst) {
                return Ok(0);
            }
            // Continue serving management commands during the restart delay.
            if let Some((mut stream, message)) = next_message(&listener, &mut pending)? {
                if message == format!("{token} stop") {
                    let _ = stream.write_all(b"OK\n");
                    return Ok(0);
                }
                if message == format!("{token} status") {
                    let _ = stream.write_all(b"STARTING\n");
                }
            }
            thread::sleep(Duration::from_millis(25));
        }
        if initial.boolean("wrapper.restart.reload_configuration", true) {
            config = options.load()?;
            ensure!(
                config.pid_file() == initial.pid_file(),
                "Changing wrapper.name or wrapper.pidfile requires stopping the wrapper"
            );
            #[cfg(unix)]
            ensure!(
                config.user() == initial.user(),
                "Changing wrapper.daemon.user or RUN_AS_USER requires stopping the wrapper"
            );
            // Finish any pending rotation before opening the next log file.
            log.flush();
            let next = Log::with_startup(
                &config,
                options.command != "daemon" && options.command != "service",
                log.startup.clone(),
            )?;
            std::mem::replace(log, next).close();
        }
    }
}
