use std::os::linux::net::SocketAddrExt;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::SocketAddr;

use anyhow::{Context, Error};
use tokio::net::UnixDatagram;

pub struct Notify {
    socket: UnixDatagram,
}

impl Notify {
    pub fn new() -> Result<Self, Error> {
        let socket_name = std::env::var_os("NOTIFY_SOCKET").context("$NOTIFY_SOCKET is not set")?;

        let addr = if socket_name.as_bytes().starts_with(b"/") {
            SocketAddr::from_pathname(&socket_name)
                .context("failed to construct the socket address from path")?
        } else if socket_name.as_bytes().starts_with(b"@") {
            SocketAddr::from_abstract_name(&socket_name.as_bytes()[1..])
                .context("failed to construct the socket address from abstract name")?
        } else {
            anyhow::bail!("unsupported socket {}", socket_name.display())
        };

        let std_socket = std::os::unix::net::UnixDatagram::unbound()
            .context("failed to create a Unix datagram socket")?;
        std_socket.connect_addr(&addr).with_context(|| format!("failed to connect to {addr:?}"))?;
        std_socket.set_nonblocking(true).context("failed to set the socket to non-blocking")?;

        let socket = UnixDatagram::from_std(std_socket)
            .context("failed to convert the socket to a Tokio socket")?;

        Ok(Self { socket })
    }

    async fn notify(&self, state: &str) -> Result<(), Error> {
        self.socket.send(state.as_bytes()).await.context("failed to send notification")?;

        Ok(())
    }

    pub async fn ready(&self) -> Result<(), Error> {
        self.notify("READY=1").await
    }

    pub async fn feed_watchdog(&self) -> Result<(), Error> {
        self.notify("WATCHDOG=1").await
    }
}
