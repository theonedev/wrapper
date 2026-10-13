#[cfg(unix)]
#[path = "unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "windows.rs"]
mod platform;
pub use platform::*;

pub fn wait_startup(
    timeout: u64,
    mut poll: impl FnMut() -> anyhow::Result<bool>,
) -> anyhow::Result<()> {
    use std::time::{Duration, Instant};
    let started = Instant::now();
    loop {
        if poll()? {
            println!("Service started");
            return Ok(());
        }
        anyhow::ensure!(
            timeout == 0 || started.elapsed() < Duration::from_secs(timeout),
            "Startup timed out"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}
