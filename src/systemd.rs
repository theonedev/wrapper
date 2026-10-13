//! Native startup notification from the foreground supervisor, without libsystemd.
use crate::{Options, supervisor};
use anyhow::{Context, Result, ensure};
use std::{
    os::{
        linux::net::SocketAddrExt,
        unix::{
            ffi::OsStrExt,
            net::{SocketAddr, UnixDatagram},
        },
    },
    time::Duration,
};

pub fn run(options: &Options) -> Result<i32> {
    let socket = std::env::var_os("NOTIFY_SOCKET")
        .map(|path| -> Result<UnixDatagram> {
            let bytes = path.as_bytes();
            let address = if let Some(name) = bytes.strip_prefix(b"@") {
                SocketAddr::from_abstract_name(name)?
            } else {
                ensure!(
                    bytes.starts_with(b"/"),
                    "Invalid systemd NOTIFY_SOCKET path"
                );
                SocketAddr::from_pathname(path)?
            };
            let socket = UnixDatagram::unbound()?;
            socket.set_write_timeout(Some(Duration::from_secs(5)))?;
            socket.connect_addr(&address)?;
            Ok(socket)
        })
        .transpose()
        .context("Connecting to systemd notification socket")?;
    supervisor::run(
        options,
        |state| {
            if let Some(socket) = &socket {
                let message = match state {
                    supervisor::State::Starting => "STATUS=Starting JVM",
                    // Match the existing startup result: the Java bridge connected,
                    // which does not imply that application initialization is complete.
                    supervisor::State::Running => "READY=1\nSTATUS=JVM bridge connected",
                    supervisor::State::Stopping => "STOPPING=1\nSTATUS=Stopping JVM",
                };
                socket
                    .send(message.as_bytes())
                    .context("Notifying systemd of service state")?;
            }
            Ok(())
        },
        None,
    )
}
