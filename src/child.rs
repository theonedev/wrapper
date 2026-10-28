use anyhow::{Context, Result};
use std::{
    io,
    process::{Child, Command, ExitStatus},
};

// When running as PID 1, the wrapper adopts orphaned descendants. Peek so the managed
// Java child remains owned by std::process::Child, including its exit status.
pub fn reap_orphans(protected_child: Option<u32>) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    if std::process::id() == 1 {
        loop {
            let mut info = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
            let result = unsafe {
                libc::waitid(
                    libc::P_ALL,
                    0,
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            if result == -1 {
                let error = io::Error::last_os_error();
                match error.raw_os_error() {
                    Some(libc::ECHILD) => break,
                    Some(libc::EINTR) => continue,
                    _ => return Err(error),
                }
            }
            let pid = unsafe { info.si_pid() };
            if pid == 0 || protected_child == Some(pid as u32) {
                break;
            }
            if unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) } == -1 {
                let error = io::Error::last_os_error();
                if !matches!(error.raw_os_error(), Some(libc::ECHILD | libc::EINTR)) {
                    return Err(error);
                }
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = protected_child;
    Ok(())
}

// Own the process group/job for both the Java version probe and the application.
pub struct ManagedChild {
    pub child: Child,
    cleaned: bool,
    #[cfg(windows)]
    job: crate::service::Job,
}
impl ManagedChild {
    pub fn spawn(command: &mut Command) -> Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn().context("Starting JVM")?;
        #[cfg(windows)]
        {
            let job = match crate::service::Job::attach(&child) {
                Ok(job) => job,
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error);
                }
            };
            Ok(Self {
                child,
                job,
                cleaned: false,
            })
        }
        #[cfg(not(windows))]
        {
            let _ = &mut child;
            Ok(Self {
                child,
                cleaned: false,
            })
        }
    }
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        reap_orphans(Some(self.child.id()))?;
        self.child.try_wait()
    }

    pub fn kill(&mut self) {
        if self.cleaned {
            return;
        }
        self.cleaned = true;
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        #[cfg(windows)]
        self.job.terminate();
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = reap_orphans(None);
    }
}
impl Drop for ManagedChild {
    fn drop(&mut self) {
        self.kill();
    }
}
