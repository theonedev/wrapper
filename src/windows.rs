use crate::{
    Options,
    config::Config,
    supervisor::{self, State},
};
use anyhow::{Context, Result, bail, ensure};
use std::{
    ffi::OsString,
    os::windows::{ffi::OsStrExt, io::AsRawHandle},
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
    Foundation::{CloseHandle, HANDLE, LocalFree},
    Security::{
        Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW,
        DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, SetFileSecurityW,
    },
    System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
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
        exit_code: ServiceExitCode::Win32(code),
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

fn service_main(_: Vec<OsString>) {
    if let Err(error) = run_service()
        && let Some(options) = OPTIONS.get()
        && let Ok(config) = options.load()
        && let Ok(log) = crate::logging::Log::new(&config, false)
    {
        log.message(&format!("Service failed: {error:#}"));
        log.close();
    }
}

fn run_service() -> Result<()> {
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
    let result = supervisor::run(options, |event| {
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
    });
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
    ServiceManager::local_computer(None::<&str>, access).context(
        "Opening Windows Service Control Manager (use an elevated prompt for install/remove)",
    )
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
    println!("{} installed", config.name());
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
        let state = service.query_status()?.current_state;
        if state != ServiceState::Stopped {
            if state != ServiceState::StopPending {
                service.stop()?;
            }
            let timeout = config.number("wrapper.shutdown.timeout", 30)?;
            wait(
                &service,
                ServiceState::Stopped,
                if timeout == 0 { 0 } else { timeout + 15 },
            )?;
        }
        Ok(0)
    } else {
        supervisor::stop(config)
    }
}
pub fn manage(options: &Options, config: &Config) -> Result<i32> {
    match options.command.as_str() {
        "install" => install(options, config),
        "installstart" => {
            install(options, config)?;
            let mut start = options.clone();
            start.command = "start".into();
            manage(&start, config)
        }
        "remove" => {
            stop(config)?;
            if let Some(service) = optional(config, ServiceAccess::DELETE)? {
                service.delete()?;
            }
            println!("{} removed", config.name());
            Ok(0)
        }
        "start" => {
            let timeout = config.number("wrapper.startup.wait.timeout", 120)?;
            let service = open(config, ServiceAccess::START | ServiceAccess::QUERY_STATUS)?;
            if service.query_status()?.current_state == ServiceState::Stopped {
                service.start::<&str>(&[])?;
            }
            wait(&service, ServiceState::Running, timeout)?;
            Ok(0)
        }
        "stop" => stop(config),
        "restart" | "condrestart" => {
            if options.command == "condrestart"
                && optional(config, ServiceAccess::QUERY_STATUS)?.is_none_or(|s| {
                    s.query_status()
                        .is_ok_and(|v| v.current_state == ServiceState::Stopped)
                })
            {
                return Ok(0);
            }
            stop(config)?;
            let mut start = options.clone();
            start.command = "start".into();
            manage(&start, config)
        }
        "status" => {
            if let Some(service) = optional(config, ServiceAccess::QUERY_STATUS)? {
                let state = service.query_status()?.current_state;
                println!("{}: {state:?}", config.name());
                Ok(if state == ServiceState::Stopped { 3 } else { 0 })
            } else if let Some(state) = supervisor::request(config, "status")? {
                println!("{}: {state}", config.name());
                Ok(0)
            } else {
                println!("{}: not installed", config.name());
                Ok(3)
            }
        }
        command => bail!("Unsupported Windows command: {command}"),
    }
}
pub fn render(_: &Options, _: &Config) -> Result<String> {
    bail!("Windows services are registered with SCM; use install")
}
