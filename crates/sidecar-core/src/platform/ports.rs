//! Port allocation. Hardcoded sidecar ports collide with other software
//! sooner or later; dynamic allocation makes the collision impossible.

use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};

use crate::domain::error::SidecarError;

/// Asks the OS for a free port by binding to port 0, then releasing it.
/// The tiny window between release and the sidecar binding is acceptable in
/// practice (the supervisor retries spawn on failure anyway).
pub fn allocate_dynamic() -> Result<u16, SidecarError> {
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .map_err(SidecarError::PortAllocation)?;
    let port = listener
        .local_addr()
        .map_err(SidecarError::PortAllocation)?
        .port();
    drop(listener);
    Ok(port)
}

/// Verifies a fixed port is currently free so a configured collision fails
/// fast with a clear error instead of an opaque sidecar crash loop.
pub fn ensure_free(port: u16) -> Result<(), SidecarError> {
    match TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)) {
        Ok(l) => {
            drop(l);
            Ok(())
        }
        Err(_) => Err(SidecarError::PortInUse { port }),
    }
}
