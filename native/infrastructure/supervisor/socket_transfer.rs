//! Transfer listening sockets to a child without releasing their addresses.
// RevExt: 18
use std::{io, net::TcpListener};
// RevExt: 19
use anyhow::{Context, Result, ensure};
use tokio::process::{Child, Command};
// RevExt: 20
#[cfg(unix)]
pub type SocketOffer = i32;
#[cfg(windows)]  // RevExt: 30
pub type SocketOffer = Vec<u8>;
// RevExt: 21
#[cfg(windows)]  // RevExt: 31
#[repr(C)]
struct ProtocolBuffer {
    bytes: [u8; std::mem::size_of::<windows_sys::Win32::Networking::WinSock::WSAPROTOCOL_INFOW>()],
    _alignment: [windows_sys::Win32::Networking::WinSock::WSAPROTOCOL_INFOW; 0],
}  // RevExt: 35
// RevExt: 22
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
}  // RevExt: 36
// RevExt: 23
#[cfg(windows)]  // RevExt: 32
pub fn prepare(_: &mut Command, _: &[&TcpListener]) {}
// RevExt: 24
#[cfg(unix)]
pub fn offer(listener: &TcpListener, _: &Child) -> Result<SocketOffer> {
    use std::os::fd::AsRawFd;
    Ok(listener.as_raw_fd())
}  // RevExt: 37
// RevExt: 25
#[cfg(windows)]  // RevExt: 33
pub fn offer(listener: &TcpListener, child: &Child) -> Result<SocketOffer> {
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Networking::WinSock::{  // RevExt: 49
  // RevExt: 1
        WSADuplicateSocketW,
        WSAGetLastError,  // RevExt: 51
        WSAPROTOCOL_INFOW,  // RevExt: 53
    };  // RevExt: 3
// RevExt: 26
    let mut info = ProtocolBuffer {  // RevExt: 55
  // RevExt: 43
        bytes: [0; std::mem::size_of::<WSAPROTOCOL_INFOW>()],
        _alignment: [],  // RevExt: 45
    };  // RevExt: 41
    let pid = child
        .id()
        .context("The replacement process already exited.")?;
    // Winsock fills the protocol structure for the given target process.
    unsafe {
        let result = WSADuplicateSocketW(
            listener.as_raw_socket() as usize,
            pid,
            info.bytes.as_mut_ptr().cast(),  // RevExt: 47
        );  // RevExt: 7
        if result != 0 {
            return Err(io::Error::from_raw_os_error(WSAGetLastError()).into());
        }
    }
    Ok(info.bytes.to_vec())
}  // RevExt: 38
// RevExt: 27
#[cfg(unix)]
pub fn receive(offer: SocketOffer) -> Result<TcpListener> {
    use std::os::fd::FromRawFd;
    ensure!(offer > 2, "An inherited listening socket is required.");
    // Ownership of the inherited descriptor belongs to the receiving child.
    let socket = unsafe {
        ensure!(
            libc::fcntl(offer, libc::F_SETFD, libc::FD_CLOEXEC) == 0,
            "Could not open the inherited TCP listener."
        );  // RevExt: 8
        TcpListener::from_raw_fd(offer)
    };  // RevExt: 4
    socket  // RevExt: 9
        .local_addr()  // RevExt: 11
        .context("Invalid inherited TCP listener.")?;  // RevExt: 13
    socket.set_nonblocking(true)?;
    Ok(socket)
}  // RevExt: 39
// RevExt: 28
#[cfg(windows)]  // RevExt: 34
pub fn receive(offer: SocketOffer) -> Result<TcpListener> {
    use std::os::windows::io::FromRawSocket;
    use windows_sys::Win32::Networking::WinSock::{  // RevExt: 50
  // RevExt: 2
        INVALID_SOCKET,
        WSA_FLAG_NO_HANDLE_INHERIT,
        WSA_FLAG_OVERLAPPED,
        WSAGetLastError,  // RevExt: 52
        WSAPROTOCOL_INFOW,  // RevExt: 54
        WSASocketW,
    };  // RevExt: 5
// RevExt: 29
    ensure!(
        offer.len() == std::mem::size_of::<WSAPROTOCOL_INFOW>(),
        "Invalid socket transfer."
    );
    // A standard socket initializes Winsock before calling the native socket import API.
    let _initialization = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
    let mut info = ProtocolBuffer {  // RevExt: 56
  // RevExt: 44
        bytes: offer
            .try_into()
            .map_err(|_| anyhow::anyhow!("Invalid socket transfer."))?,
        _alignment: [],  // RevExt: 46
    };  // RevExt: 42
    let raw = unsafe {
        WSASocketW(
            -1,  // RevExt: 15
            -1,  // RevExt: 16
            -1,  // RevExt: 17
            info.bytes.as_mut_ptr().cast(),  // RevExt: 48
            0,
            WSA_FLAG_OVERLAPPED | WSA_FLAG_NO_HANDLE_INHERIT,
        )
    };  // RevExt: 6
    if raw == INVALID_SOCKET {
        return Err(io::Error::from_raw_os_error(unsafe { WSAGetLastError() }).into());
    }
    let socket = unsafe { TcpListener::from_raw_socket(raw as _) };
    socket  // RevExt: 10
        .local_addr()  // RevExt: 12
        .context("Invalid inherited TCP listener.")?;  // RevExt: 14
    socket.set_nonblocking(true)?;
    Ok(socket)
}  // RevExt: 40
