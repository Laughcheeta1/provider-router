//! Transfer listening sockets to a child without releasing their addresses.
use anyhow::{Context, Result, ensure};
use std::{io, net::TcpListener};
use tokio::process::{Child, Command};
#[cfg(unix)]
pub type SocketOffer = i32;
#[cfg(windows)]
pub type SocketOffer = Vec<u8>;
#[cfg(windows)]
#[repr(C)]
struct ProtocolBuffer {
    bytes: [u8; std::mem::size_of::<windows_sys::Win32::Networking::WinSock::WSAPROTOCOL_INFOW>()],
    _alignment: [windows_sys::Win32::Networking::WinSock::WSAPROTOCOL_INFOW; 0],
}
#[cfg(unix)]
pub fn prepare(command: &mut Command, listeners: &[&TcpListener]) {
    use std::os::fd::AsRawFd;
    let descriptors: Vec<_> = listeners.iter().map(|socket| socket.as_raw_fd()).collect();
    // The child inherits independent references. Do not change the parent's close-on-exec flags.
    unsafe {
        command.pre_exec(move || {
            for fd in &descriptors {
                if libc::fcntl(*fd, libc::F_SETFD, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
}
#[cfg(windows)]
pub fn prepare(_: &mut Command, _: &[&TcpListener]) {}
#[cfg(unix)]
pub fn offer(listener: &TcpListener, _: &Child) -> Result<SocketOffer> {
    use std::os::fd::AsRawFd;
    Ok(listener.as_raw_fd())
}
#[cfg(windows)]
pub fn offer(listener: &TcpListener, child: &Child) -> Result<SocketOffer> {
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Networking::WinSock::{
        WSADuplicateSocketW, WSAGetLastError, WSAPROTOCOL_INFOW,
    };
    let mut info = ProtocolBuffer {
        bytes: [0; std::mem::size_of::<WSAPROTOCOL_INFOW>()],
        _alignment: [],
    };
    let pid = child
        .id()
        .context("The replacement process already exited.")?;
    // Winsock fills the protocol structure for the given target process.
    unsafe {
        let result = WSADuplicateSocketW(
            listener.as_raw_socket() as usize,
            pid,
            info.bytes.as_mut_ptr().cast(),
        );
        if result != 0 {
            return Err(io::Error::from_raw_os_error(WSAGetLastError()).into());
        }
    }
    Ok(info.bytes.to_vec())
}
#[cfg(unix)]
pub fn receive(offer: SocketOffer) -> Result<TcpListener> {
    use std::os::fd::FromRawFd;
    ensure!(offer > 2, "An inherited listening socket is required.");
    // Ownership of the inherited descriptor belongs to the receiving child.
    let socket = unsafe {
        ensure!(
            libc::fcntl(offer, libc::F_SETFD, libc::FD_CLOEXEC) == 0,
            "Could not open the inherited TCP listener."
        );
        TcpListener::from_raw_fd(offer)
    };
    socket
        .local_addr()
        .context("Invalid inherited TCP listener.")?;
    socket.set_nonblocking(true)?;
    Ok(socket)
}
#[cfg(windows)]
pub fn receive(offer: SocketOffer) -> Result<TcpListener> {
    use std::os::windows::io::FromRawSocket;
    use windows_sys::Win32::Networking::WinSock::{
        INVALID_SOCKET, WSA_FLAG_NO_HANDLE_INHERIT, WSA_FLAG_OVERLAPPED, WSAGetLastError,
        WSAPROTOCOL_INFOW, WSASocketW,
    };
    ensure!(
        offer.len() == std::mem::size_of::<WSAPROTOCOL_INFOW>(),
        "Invalid socket transfer."
    );
    // A standard socket initializes Winsock before calling the native socket import API.
    let _initialization = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
    let mut info = ProtocolBuffer {
        bytes: offer
            .try_into()
            .map_err(|_| anyhow::anyhow!("Invalid socket transfer."))?,
        _alignment: [],
    };
    let raw = unsafe {
        WSASocketW(
            -1,
            -1,
            -1,
            info.bytes.as_mut_ptr().cast(),
            0,
            WSA_FLAG_OVERLAPPED | WSA_FLAG_NO_HANDLE_INHERIT,
        )
    };
    if raw == INVALID_SOCKET {
        return Err(io::Error::from_raw_os_error(unsafe { WSAGetLastError() }).into());
    }
    let socket = unsafe { TcpListener::from_raw_socket(raw as _) };
    socket
        .local_addr()
        .context("Invalid inherited TCP listener.")?;
    socket.set_nonblocking(true)?;
    Ok(socket)
}
