use anyhow::{Context, Result};
use std::process::{Child, Command};

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
    }
}
impl Drop for ManagedChild {
    fn drop(&mut self) {
        self.kill();
    }
}
