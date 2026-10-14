use crate::{
    Options,
    config::Config,
    supervisor::{self, State},
};
use anyhow::{Context, Result, bail, ensure};
use std::{
    ffi::OsString,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
    process::Child,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use windows_service::{
    define_windows_service,
    service::{
        Service, ServiceAccess, ServiceControl, ServiceControlAccept, ServiceDependency,
        ServiceErrorControl, ServiceExitCode, ServiceInfo, ServiceStartType, ServiceState,
        ServiceStatus, ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult},
    service_dispatcher,
    service_manager::{ServiceManager, ServiceManagerAccess},
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, ERROR_ACCESS_DENIED, ERROR_CANCELLED, HANDLE, LocalFree, WAIT_OBJECT_0,
        WAIT_TIMEOUT,
    },
    Security::{
        Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW,
        DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, SetFileSecurityW,
    },
    System::{
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject, TerminateJobObject,
        },
        Threading::{GetExitCodeProcess, WaitForSingleObject},
    },
    UI::{
        Shell::{
            IsUserAnAdmin, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
            ShellExecuteExW,
        },
        WindowsAndMessaging::SW_SHOWNORMAL,
    },
};

pub struct Job(HANDLE);
impl Job {
    pub fn attach(child: &Child) -> Result<Self> {
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            ensure!(
                !handle.is_null(),
                "Creating JVM job: {}",
                std::io::Error::last_os_error()
            );
            let job = Self(handle);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            ensure!(
                SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as _,
                    std::mem::size_of_val(&info) as u32
                ) != 0,
                "Configuring JVM job: {}",
                std::io::Error::last_os_error()
            );
            ensure!(
                AssignProcessToJobObject(handle, child.as_raw_handle() as HANDLE) != 0,
                "Assigning JVM job: {}",
                std::io::Error::last_os_error()
            );
            Ok(job)
        }
    }
    pub fn terminate(&self) {
        unsafe {
            TerminateJobObject(self.0, 1);
        }
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

pub fn protect_control_file(path: &Path) -> Result<()> {
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // Only the owner, Administrators and LocalSystem may read the authentication token.
    let sddl: Vec<u16> = "D:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FA;;;OW)"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    unsafe {
        let mut descriptor = std::ptr::null_mut();
        ensure!(
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                std::ptr::null_mut()
            ) != 0,
            "Creating control-file ACL: {}",
            std::io::Error::last_os_error()
        );
        let ok = SetFileSecurityW(
            path.as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor,
        );
        let error = std::io::Error::last_os_error();
        LocalFree(descriptor);
        ensure!(ok != 0, "Protecting control file: {error}");
    }
    Ok(())
}

static OPTIONS: OnceLock<Options> = OnceLock::new();
define_windows_service!(ffi_service_main, service_main);

fn status(state: ServiceState, checkpoint: u32, code: u32) -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: if state == ServiceState::Running {
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
        } else {
            ServiceControlAccept::empty()
        },
        // JVM/application exit codes are not Win32 error numbers (1 means
        // "Incorrect function" to Windows, which hides the real startup failure).
        exit_code: if code == 0 {
            ServiceExitCode::Win32(0)
        } else {
            ServiceExitCode::ServiceSpecific(code)
        },
        checkpoint,
        wait_hint: if matches!(
            state,
            ServiceState::StartPending | ServiceState::StopPending
        ) {
            Duration::from_secs(15)
        } else {
            Duration::ZERO
        },
        process_id: None,
    }
}

fn service_main(arguments: Vec<OsString>) {
    let startup = if arguments
        .get(1)
        .is_some_and(|arg| arg == "--startup-notify")
    {
        arguments
            .get(2)
            .and_then(|arg| arg.to_str())
            .and_then(|port| port.parse().ok())
            .zip(arguments.get(3).and_then(|arg| arg.to_str()))
            .and_then(|(port, token)| crate::startup::Capture::connect(port, token).ok())
    } else {
        None
    };
    if let Err(error) = run_service(startup.clone()) {
        let message = format!("Service failed: {error:#}");
        if let Some(startup) = &startup {
            startup.record(message.as_bytes());
        }
        if let Some(options) = OPTIONS.get()
            && let Ok(config) = options.load()
            && let Ok(log) = crate::logging::Log::new(&config, false)
        {
            log.message(&message);
            log.close();
        }
    }
    if let Some(startup) = startup {
        let _ = startup.finish();
    }
}

fn run_service(startup: Option<crate::startup::Capture>) -> Result<()> {
    let options = OPTIONS.get().context("Missing service options")?;
    let config = options.load()?;
    let state = Arc::new(Mutex::new(ServiceState::StartPending));
    let stopped = Arc::new(AtomicBool::new(false));
    let handle = service_control_handler::register(config.name(), move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            supervisor::STOP.store(true, Ordering::SeqCst);
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })?;
    handle.set_service_status(status(ServiceState::StartPending, 1, 0))?;
    let worker_state = state.clone();
    let worker_stopped = stopped.clone();
    // Keep SCM informed during slow startup and unlimited graceful shutdown.
    let reporter = thread::spawn(move || {
        let mut checkpoint = 2;
        while !worker_stopped.load(Ordering::SeqCst) {
            let current = worker_state.lock().unwrap();
            if matches!(
                *current,
                ServiceState::StartPending | ServiceState::StopPending
            ) {
                let _ = handle.set_service_status(status(*current, checkpoint, 0));
                checkpoint += 1;
            }
            drop(current);
            thread::sleep(Duration::from_secs(1));
        }
    });
    let mut ever_running = false;
    let result = supervisor::run(
        options,
        |event| {
            let current = match event {
                State::Starting if ever_running => ServiceState::Running,
                State::Starting => ServiceState::StartPending,
                State::Running => {
                    ever_running = true;
                    ServiceState::Running
                }
                State::Stopping => ServiceState::StopPending,
            };
            let mut guard = state.lock().unwrap();
            *guard = current;
            handle.set_service_status(status(
                current,
                if matches!(
                    current,
                    ServiceState::StartPending | ServiceState::StopPending
                ) {
                    1
                } else {
                    0
                },
                0,
            ))?;
            Ok(())
        },
        startup,
    );
    stopped.store(true, Ordering::SeqCst);
    let _ = reporter.join();
    let code = match &result {
        Ok(code) => *code as u32,
        Err(_) => 1,
    };
    handle.set_service_status(status(ServiceState::Stopped, 0, code))?;
    result.map(|_| ())
}

pub fn dispatch(options: &Options, config: &Config) -> Result<i32> {
    OPTIONS
        .set(options.clone())
        .map_err(|_| anyhow::anyhow!("Service dispatcher already initialized"))?;
    service_dispatcher::start(config.name(), ffi_service_main)?;
    Ok(0)
}

fn manager(create: bool) -> Result<ServiceManager> {
    let access = ServiceManagerAccess::CONNECT
        | if create {
            ServiceManagerAccess::CREATE_SERVICE
        } else {
            ServiceManagerAccess::empty()
        };
    ServiceManager::local_computer(None::<&str>, access)
        .context("Opening Windows Service Control Manager")
}
fn open(config: &Config, access: ServiceAccess) -> Result<Service> {
    Ok(manager(false)?.open_service(config.name(), access)?)
}
fn optional(config: &Config, access: ServiceAccess) -> Result<Option<Service>> {
    match manager(false)?.open_service(config.name(), access) {
        Ok(service) => Ok(Some(service)),
        Err(windows_service::Error::Winapi(error)) if error.raw_os_error() == Some(1060) => {
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

fn install(options: &Options, config: &Config) -> Result<i32> {
    if optional(config, ServiceAccess::QUERY_STATUS)?.is_some() {
        println!("{} is already installed", config.name());
        return Ok(0);
    }
    println!("Installing service...");
    let start_type = match config.get("wrapper.ntservice.starttype", "AUTO_START") {
        "AUTO_START" | "DELAY_START" => ServiceStartType::AutoStart,
        "DEMAND_START" => ServiceStartType::OnDemand,
        value => bail!("Unsupported wrapper.ntservice.starttype: {value}"),
    };
    let dependencies = config
        .numbered("wrapper.ntservice.dependency.")
        .into_iter()
        .map(|v| {
            if let Some(group) = v.strip_prefix('+') {
                ServiceDependency::Group(group.into())
            } else {
                ServiceDependency::Service(v.into())
            }
        })
        .collect();
    // Persist config location, not credentials or environment-expanded JVM settings.
    let info = ServiceInfo {
        name: config.name().into(),
        display_name: config.get("wrapper.displayname", config.name()).into(),
        service_type: ServiceType::OWN_PROCESS,
        start_type,
        error_control: ServiceErrorControl::Normal,
        executable_path: std::env::current_exe()?,
        launch_arguments: options
            .service_args("-s")
            .into_iter()
            .map(OsString::from)
            .collect(),
        dependencies,
        account_name: nonempty(config.get("wrapper.ntservice.account", "")),
        account_password: nonempty(config.get("wrapper.ntservice.password", "")),
    };
    let service = manager(true)?
        .create_service(&info, ServiceAccess::CHANGE_CONFIG | ServiceAccess::DELETE)?;
    let configure = (|| -> Result<()> {
        service.set_description(config.get("wrapper.description", config.name()))?;
        service.set_delayed_auto_start(
            config.get("wrapper.ntservice.starttype", "AUTO_START") == "DELAY_START",
        )?;
        Ok(())
    })();
    if configure.is_err() {
        let _ = service.delete();
    }
    configure?;
    println!("Service installed");
    Ok(0)
}
fn nonempty(value: &str) -> Option<OsString> {
    (!value.is_empty()).then(|| value.into())
}

fn wait(service: &Service, desired: ServiceState, timeout: u64) -> Result<()> {
    let start = Instant::now();
    loop {
        let state = service.query_status()?;
        if state.current_state == desired {
            return Ok(());
        }
        ensure!(
            desired != ServiceState::Running || state.current_state != ServiceState::Stopped,
            "Service failed to start: {:?}",
            state.exit_code
        );
        ensure!(
            timeout == 0 || start.elapsed() < Duration::from_secs(timeout),
            "Timed out waiting for service; current state: {:?}",
            state.current_state
        );
        thread::sleep(Duration::from_millis(200));
    }
}
fn stop(config: &Config) -> Result<i32> {
    if let Some(service) = optional(config, ServiceAccess::STOP | ServiceAccess::QUERY_STATUS)? {
        println!("Stopping service...");
        let timeout = config.number("wrapper.shutdown.timeout", 30)?;
        let start = Instant::now();
        let mut requested = false;
        loop {
            let state = service.query_status()?.current_state;
            if state == ServiceState::Stopped {
                break;
            }
            if !requested {
                match state {
                    ServiceState::StartPending => {
                        // SCM rejects Stop during startup. Use the authenticated
                        // supervisor channel, which also interrupts share retry waits.
                        // None can mean the service has not published its control
                        // file yet; recheck SCM and retry instead of reporting success.
                        requested = supervisor::request(config, "stop")?.is_some();
                    }
                    ServiceState::StopPending => requested = true,
                    _ => {
                        service.stop()?;
                        requested = true;
                    }
                }
            }
            ensure!(
                timeout == 0 || start.elapsed() < Duration::from_secs(timeout.saturating_add(15)),
                "Timed out waiting for service; current state: {state:?}"
            );
            thread::sleep(Duration::from_millis(200));
        }
        println!("Service stopped");
        Ok(0)
    } else {
        supervisor::stop(config, false)
    }
}
// Quote for Windows argv parsing, not cmd.exe: no shell interprets these arguments.
fn quote_argument(value: &str) -> String {
    let mut quoted = String::from("\"");
    let mut slashes = 0;
    for ch in value.chars() {
        if ch == '\\' {
            slashes += 1;
        } else {
            quoted.push_str(&"\\".repeat(if ch == '"' { slashes * 2 + 1 } else { slashes }));
            quoted.push(ch);
            slashes = 0;
        }
    }
    quoted.push_str(&"\\".repeat(slashes * 2));
    quoted.push('"');
    quoted
}

fn elevate(options: &Options) -> Result<i32> {
    let mut report = crate::startup::SocketWait::new()?;
    let [port, token] = report.arguments()?;
    let executable: Vec<u16> = std::env::current_exe()?
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let directory: Vec<u16> = std::env::current_dir()?
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // Keep overrides (including installation credentials) and application arguments.
    let mut arguments = vec!["--elevated-result".to_owned(), port, token];
    arguments.extend(options.child_args(&options.command));
    let parameters: Vec<u16> = arguments
        .iter()
        .map(|arg| quote_argument(arg))
        .collect::<Vec<_>>()
        .join(" ")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let verb: Vec<u16> = "runas\0".encode_utf16().collect();
    println!(
        "Requesting administrator permission to {} service...",
        options.command
    );
    unsafe {
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = std::mem::size_of_val(&info) as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
        info.lpVerb = verb.as_ptr();
        info.lpFile = executable.as_ptr();
        info.lpParameters = parameters.as_ptr();
        info.lpDirectory = directory.as_ptr();
        info.nShow = SW_SHOWNORMAL;
        if ShellExecuteExW(&mut info) == 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_CANCELLED as i32) {
                bail!("Administrator permission was cancelled");
            }
            return Err(error).context("Requesting administrator permission");
        }
        ensure!(!info.hProcess.is_null(), "Elevated process handle missing");
        let process = OwnedHandle::from_raw_handle(info.hProcess);
        let diagnostic = wait_elevated(&process, &mut report)?;
        let mut code = 0;
        ensure!(
            GetExitCodeProcess(process.as_raw_handle(), &mut code) != 0,
            "Reading elevated command result: {}",
            std::io::Error::last_os_error()
        );
        if code == 0 {
            println!("Elevated {} completed", options.command);
        } else if let Some(Err(error)) = diagnostic {
            eprintln!("Wrapper: {error:#}");
        } else {
            eprintln!("Elevated {} failed with exit code {code}", options.command);
        }
        Ok(code as i32)
    }
}

// Drain diagnostics while waiting so even a full report cannot block the child.
// The process handle remains authoritative for the command's exit status.
fn wait_elevated(
    process: &OwnedHandle,
    report: &mut crate::startup::SocketWait,
) -> Result<Option<Result<()>>> {
    let mut diagnostic = None;
    let mut exited = None;
    loop {
        if diagnostic.is_none() {
            diagnostic = match report.poll_command() {
                Ok(true) => Some(Ok(())),
                Ok(false) => None,
                Err(error) => Some(Err(error)),
            };
        }
        if let Some(exited) = exited {
            // Allow buffered TCP data to arrive after process exit, but retain
            // a fallback for a child that crashes before reporting anything.
            if diagnostic.is_some() || Instant::now() >= exited {
                return Ok(diagnostic);
            }
            thread::sleep(Duration::from_millis(25));
        } else {
            match unsafe { WaitForSingleObject(process.as_raw_handle(), 25) } {
                WAIT_OBJECT_0 => exited = Some(Instant::now() + Duration::from_secs(2)),
                WAIT_TIMEOUT => (),
                _ => {
                    return Err(std::io::Error::last_os_error())
                        .context("Waiting for elevated command");
                }
            }
        }
    }
}

// This entry point is invoked by ShellExecuteEx after UAC consent. Its one-time
// authenticated channel reports errors even when logging is disabled or broken.
pub fn run_elevated(arguments: &[String]) -> Result<i32> {
    ensure!(
        arguments.len() >= 3,
        "Missing elevated command result arguments"
    );
    let report = crate::startup::Capture::connect(arguments[0].parse()?, &arguments[1])?;
    let result = crate::run_args(arguments[2..].to_vec());
    match result {
        Ok(0) => {
            report.ready()?;
            Ok(0)
        }
        Ok(code) => {
            report.record(format!("Elevated command failed with exit code {code}").as_bytes());
            report.finish()?;
            Ok(code)
        }
        Err(error) => {
            report.record(format!("{error:#}").as_bytes());
            report.finish()?;
            Ok(1)
        }
    }
}

fn should_elevate(command: &str, error: &anyhow::Error, administrator: bool) -> bool {
    !administrator
        && matches!(command, "start" | "stop" | "restart" | "condrestart")
        && error.chain().any(|cause| {
            cause
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32))
        })
}

pub fn manage(options: &Options, config: &Config) -> Result<i32> {
    if matches!(
        options.command.as_str(),
        "install" | "installstart" | "remove"
    ) && unsafe { IsUserAnAdmin() } == 0
    {
        return elevate(options);
    }
    let result = manage_command(options, config);
    if let Err(error) = &result
        && should_elevate(&options.command, error, unsafe { IsUserAnAdmin() } != 0)
    {
        return elevate(options);
    }
    result
}

fn manage_command(options: &Options, config: &Config) -> Result<i32> {
    match options.command.as_str() {
        "install" => install(options, config),
        "installstart" => {
            install(options, config)?;
            let mut start = options.clone();
            start.command = "start".into();
            manage_command(&start, config)
        }
        "remove" => {
            println!("Removing service...");
            stop(config)?;
            if let Some(service) = optional(config, ServiceAccess::DELETE)? {
                service.delete()?;
            }
            println!("Service removed");
            Ok(0)
        }
        "start" => {
            let timeout = config.number("wrapper.startup.wait.timeout", 120)?;
            let service = open(config, ServiceAccess::START | ServiceAccess::QUERY_STATUS)?;
            let state = service.query_status()?.current_state;
            if state == ServiceState::Running {
                println!("Service is already running");
                return Ok(0);
            }
            println!("Starting service...");
            if state == ServiceState::Stopped {
                let mut startup = crate::startup::SocketWait::new()?;
                let [port, token] = startup.arguments()?;
                service.start(&["--startup-notify", &port, &token])?;
                super::wait_startup(timeout, || {
                    if startup.poll()? {
                        return Ok(true);
                    }
                    let state = service.query_status()?;
                    if state.current_state == ServiceState::Stopped {
                        // Drain a result sent just before SCM reported Stopped.
                        if startup.poll()? {
                            return Ok(true);
                        }
                        // TCP may split a diagnostic across multiple reads even
                        // after the service has finished writing it.
                        if !startup.has_pending() {
                            bail!("Service failed to start: {:?}", state.exit_code);
                        }
                    }
                    Ok(false)
                })?;
            } else {
                // Another caller (or boot) initiated this startup. SCM marks the
                // service Running only after the same JVM handshake.
                wait(&service, ServiceState::Running, timeout)?;
                println!("Service started");
            }
            Ok(0)
        }
        "stop" => stop(config),
        "restart" | "condrestart" => {
            if options.command == "condrestart" {
                let Some(service) = optional(config, ServiceAccess::QUERY_STATUS)? else {
                    return Ok(0);
                };
                if service.query_status()?.current_state == ServiceState::Stopped {
                    return Ok(0);
                }
            }
            // Check both rights before stopping: a denied start must not leave a
            // service stopped before UAC approval, especially for condrestart.
            let _service = optional(
                config,
                ServiceAccess::START | ServiceAccess::STOP | ServiceAccess::QUERY_STATUS,
            )?;
            stop(config)?;
            let mut start = options.clone();
            start.command = "start".into();
            manage_command(&start, config)
        }
        "status" => {
            if let Some(service) = optional(config, ServiceAccess::QUERY_STATUS)? {
                let state = service.query_status()?.current_state;
                println!("{}", format!("{state:?}").to_ascii_uppercase());
                Ok(if state == ServiceState::Stopped { 3 } else { 0 })
            } else if let Some(state) = supervisor::request(config, "status")? {
                println!("{state}");
                Ok(0)
            } else {
                println!("STOPPED");
                Ok(3)
            }
        }
        command => bail!("Unsupported Windows command: {command}"),
    }
}
pub fn render(_: &Options, _: &Config) -> Result<String> {
    bail!("Windows services are registered with SCM; use install")
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::Shell::CommandLineToArgvW;

    #[test]
    fn elevated_command_reports_missing_java_without_log_files() {
        let mut receiver = crate::startup::SocketWait::new().unwrap();
        let [port, token] = receiver.arguments().unwrap();
        let path =
            std::env::temp_dir().join(format!("wrapper-uac-check-{}.conf", std::process::id()));
        std::fs::write(
            &path,
            "wrapper.java.command=missing-java-for-uac-check.exe\nwrapper.logfile.loglevel=NONE\n",
        )
        .unwrap();
        let result = run_elevated(&[
            port,
            token,
            "check".into(),
            path.to_string_lossy().into_owned(),
        ]);
        std::fs::remove_file(path).unwrap();
        assert_eq!(result.unwrap(), 1);
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match receiver.poll_command() {
                Err(error) => {
                    let message = format!("{error:#}");
                    assert!(
                        message.contains("Launching missing-java-for-uac-check.exe"),
                        "{message}"
                    );
                    assert!(message.contains("Starting JVM"), "{message}");
                    break;
                }
                Ok(ready) => assert!(!ready),
            }
            assert!(
                Instant::now() < deadline,
                "Missing elevated command diagnostic"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn application_failure_is_not_a_win32_error() {
        assert_eq!(
            status(ServiceState::Stopped, 0, 1).exit_code,
            ServiceExitCode::ServiceSpecific(1)
        );
        assert_eq!(
            status(ServiceState::Stopped, 0, 0).exit_code,
            ServiceExitCode::Win32(0)
        );
    }

    #[test]
    fn elevation_requires_access_denied_on_a_management_command() {
        let error = anyhow::Error::new(windows_service::Error::Winapi(
            std::io::Error::from_raw_os_error(ERROR_ACCESS_DENIED as i32),
        ))
        .context("Opening service");
        for command in ["start", "stop", "restart", "condrestart"] {
            assert!(should_elevate(command, &error, false));
            assert!(!should_elevate(command, &error, true));
        }
        for command in ["console", "status", "check", "service", "unknown"] {
            assert!(!should_elevate(command, &error, false));
        }
        let missing_service = anyhow::Error::new(windows_service::Error::Winapi(
            std::io::Error::from_raw_os_error(1060),
        ));
        assert!(!should_elevate("start", &missing_service, false));
        assert!(!should_elevate(
            "start",
            &anyhow::anyhow!("Access is denied"),
            false
        ));
    }

    #[test]
    fn elevated_arguments_round_trip() {
        let arguments = [
            "wrapper.exe",
            "install",
            r"C:\Program Files\OneDev\conf\wrapper.conf",
            "",
            r"C:\trailing path\",
            r#"quoted \"value\""#,
            "wrapper.ntservice.password=a&b%PATH%!",
            "--",
            "Unicode \u{670d}\u{52a1}",
        ];
        let command: Vec<u16> = arguments
            .iter()
            .map(|arg| quote_argument(arg))
            .collect::<Vec<_>>()
            .join(" ")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        unsafe {
            let mut count = 0;
            let argv = CommandLineToArgvW(command.as_ptr(), &mut count);
            assert!(!argv.is_null());
            let decoded: Vec<String> = std::slice::from_raw_parts(argv, count as usize)
                .iter()
                .map(|arg| {
                    let mut length = 0;
                    while *arg.add(length) != 0 {
                        length += 1;
                    }
                    String::from_utf16_lossy(std::slice::from_raw_parts(*arg, length))
                })
                .collect();
            LocalFree(argv as _);
            assert_eq!(decoded, arguments);
        }
    }
}
