//! Check that the container listener address belongs to a Linux Docker bridge and admit only Docker peers.
// RevExt: 1
use std::net::{IpAddr, SocketAddr};
// RevExt: 13
use tokio::net::TcpListener;
// RevExt: 2
use anyhow::Result;
// RevExt: 3
use crate::domain::Network;
// RevExt: 4
pub(super) fn container_bridge_address(network: &Network) -> Result<Option<SocketAddr>> {
    let Some(address) = network.container_bridge_address()? else {
        return Ok(None);
    };
    #[cfg(not(target_os = "linux"))]
    anyhow::bail!("Container bridge listeners are supported on Linux only ({address}).");
    #[cfg(target_os = "linux")]
    {
        let ip = address.ip();
        validate_bridge_interfaces(ip, &interface_names(ip)?, is_bridge_device)?;
        Ok(Some(address))
    } // RevExt: 16
} // RevExt: 23
// RevExt: 5
#[cfg(target_os = "linux")] // RevExt: 14
fn validate_bridge_interfaces(
    ip: IpAddr,
    names: &[impl AsRef<str>],
    is_bridge_device: impl Fn(&str) -> bool,
) -> Result<()> {
    anyhow::ensure!(
        !names.is_empty(),
        "Container bridge host {ip} is not assigned to a local network interface."
    );
    // Reject duplicate assignments if any interface with this IP is outside Docker.
    for name in names {
        let name = name.as_ref();
        anyhow::ensure!(
            is_docker_bridge_name(name) && is_bridge_device(name),
            "Container bridge host {ip} belongs to {name}, not a Docker bridge."
        );
    } // RevExt: 17
    Ok(())
} // RevExt: 24
// RevExt: 6
/// Docker names its bridges `docker0` or `br-` followed by the first 12 hex digits of the network ID.
#[cfg(target_os = "linux")] // RevExt: 39
fn is_docker_bridge_name(name: &str) -> bool {
    name == "docker0"
        || name.strip_prefix("br-").is_some_and(|id| {
            id.len() == 12
                && id
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        }) // RevExt: 67
} // RevExt: 40
// RevExt: 36
/// Only Linux bridge devices expose bridge attributes in sysfs.
#[cfg(target_os = "linux")] // RevExt: 45
fn is_bridge_device(name: &str) -> bool {
    // RevExt: 43
    std::path::Path::new("/sys/class/net")
        .join(name)
        .join("bridge")
        .is_dir()
} // RevExt: 41
// RevExt: 37
// RevExt: 44
// RevExt: 42
// RevExt: 38
/// Accept only connections from addresses inside a local Docker bridge network, whatever host routes exist.
pub(super) struct DockerPeers(pub(super) TcpListener);
// RevExt: 46
impl axum::serve::Listener for DockerPeers {
    type Io = tokio::net::TcpStream;
    type Addr = SocketAddr;
    // RevExt: 47
    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let (stream, peer) = axum::serve::Listener::accept(&mut self.0).await;
            if is_docker_peer(peer.ip()) {
                return (stream, peer);
            } // RevExt: 69
        } // RevExt: 72
    } // RevExt: 55
    // RevExt: 48
    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.0.local_addr()
    } // RevExt: 56
} // RevExt: 59
// RevExt: 49
#[cfg(target_os = "linux")] // RevExt: 64
fn is_docker_peer(peer: IpAddr) -> bool {
    let peer = peer.to_canonical();
    interface_addresses().is_ok_and(|addresses| {
        addresses.iter().any(|(name, ip, netmask)| {
            netmask.is_some_and(|netmask| in_network(peer, *ip, netmask))
                && is_docker_bridge_name(name)
                && is_bridge_device(name)
        }) // RevExt: 68
    })
} // RevExt: 60
// RevExt: 50
#[cfg(not(target_os = "linux"))]
fn is_docker_peer(_peer: IpAddr) -> bool {
    false
} // RevExt: 61
// RevExt: 51
#[cfg(target_os = "linux")] // RevExt: 65
fn in_network(peer: IpAddr, ip: IpAddr, netmask: IpAddr) -> bool {
    match (peer, ip, netmask) {
        (IpAddr::V4(peer), IpAddr::V4(ip), IpAddr::V4(mask)) => {
            u32::from(peer) & u32::from(mask) == u32::from(ip) & u32::from(mask)
        } // RevExt: 73
        (IpAddr::V6(peer), IpAddr::V6(ip), IpAddr::V6(mask)) => {
            u128::from(peer) & u128::from(mask) == u128::from(ip) & u128::from(mask)
        } // RevExt: 74
        _ => false,
    } // RevExt: 57
} // RevExt: 62
// RevExt: 52
#[cfg(target_os = "linux")] // RevExt: 66
fn interface_names(ip: IpAddr) -> Result<Vec<String>> {
    Ok(interface_addresses()?
        .into_iter()
        .filter(|(_, interface_ip, _)| *interface_ip == ip)
        .map(|(name, _, _)| name)
        .collect())
} // RevExt: 63
// RevExt: 53
/// List each interface address as its name, IP and netmask.
#[cfg(target_os = "linux")] // RevExt: 15
fn interface_addresses() -> Result<Vec<(String, IpAddr, Option<IpAddr>)>> {
    use std::{
        ffi::CStr,
        io,
        net::{Ipv4Addr, Ipv6Addr},
        ptr,
    };
    // RevExt: 7
    use anyhow::Context;
    // RevExt: 8
    let mut interfaces = ptr::null_mut();
    // getifaddrs initializes the list on success.
    if unsafe { libc::getifaddrs(&mut interfaces) } != 0 {
        return Err(io::Error::last_os_error())
            .context("Could not inspect local network interfaces.");
    } // RevExt: 18
    let mut addresses = Vec::new();
    let mut entry = interfaces;
    // Each address belongs to the live list and its family determines its struct type.
    while let Some(interface) = unsafe { entry.as_ref() } {
        entry = interface.ifa_next;
        let Some(address) = (unsafe { interface.ifa_addr.as_ref() }) else {
            continue; // RevExt: 75
        }; // RevExt: 27
        // The netmask shares the address family of its interface address.
        let ip_of = |pointer: *mut libc::sockaddr| match i32::from(address.sa_family) {
            libc::AF_INET => {
                let address = unsafe { &*pointer.cast::<libc::sockaddr_in>() };
                Some(IpAddr::V4(Ipv4Addr::from(
                    address.sin_addr.s_addr.to_ne_bytes(),
                )))
            } // RevExt: 70
            libc::AF_INET6 => {
                let address = unsafe { &*pointer.cast::<libc::sockaddr_in6>() };
                Some(IpAddr::V6(Ipv6Addr::from(address.sin6_addr.s6_addr)))
            } // RevExt: 71
            _ => None,
        }; // RevExt: 28
        let Some(interface_ip) = ip_of(interface.ifa_addr) else {
            continue; // RevExt: 76
        }; // RevExt: 77
        let netmask = (!interface.ifa_netmask.is_null())
            .then(|| ip_of(interface.ifa_netmask))
            .flatten();
        // getifaddrs provides a NUL-terminated name for the lifetime of the list.
        let name = unsafe { CStr::from_ptr(interface.ifa_name) };
        addresses.push((name.to_string_lossy().into_owned(), interface_ip, netmask));
        // RevExt: 29
    } // RevExt: 19
    // The list was allocated by getifaddrs and is no longer in use.
    unsafe { libc::freeifaddrs(interfaces) };
    Ok(addresses)
} // RevExt: 25
// RevExt: 9
#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    // RevExt: 10
    #[test] // RevExt: 31
    fn accepts_default_and_user_defined_docker_bridges() -> Result<()> {
        validate_bridge_interfaces("172.17.0.1".parse()?, &["docker0"], |_| true)?;
        validate_bridge_interfaces("fd00::1".parse()?, &["br-a1b2c3d4e5f6"], |_| true)
    } // RevExt: 20
    // RevExt: 11
    #[test] // RevExt: 32
    fn rejects_private_addresses_on_other_interfaces() -> Result<()> {
        let ip = "192.168.1.20".parse()?;
        for name in [
            "eth0",
            "wlan0",
            "br-",
            "br-lan",
            "br-A1B2C3D4E5F6",
            "br-a1b2c3d4e5f6a",
            "br-0123456789ab",
        ] {
            let error = validate_bridge_interfaces(ip, &[name], |name| name != "br-0123456789ab")
                .unwrap_err();  // RevExt: 80
            assert!(
                error
                    .to_string()
                    .contains(&format!("{ip} belongs to {name}, not a Docker bridge"))
            );
        } // RevExt: 30
        Ok(()) // RevExt: 34
    } // RevExt: 21
    // RevExt: 12
    #[test] // RevExt: 78
    fn docker_peers_must_be_inside_the_bridge_network() -> Result<()> {
        let (bridge, mask) = ("172.18.0.1".parse()?, "255.255.0.0".parse()?);
        assert!(in_network("172.18.0.5".parse()?, bridge, mask));
        assert!(!in_network("10.8.0.5".parse()?, bridge, mask));
        assert!(!in_network("172.19.0.5".parse()?, bridge, mask));
        assert!(!in_network("fd00::5".parse()?, bridge, mask));
        Ok(()) // RevExt: 79
    } // RevExt: 58
    // RevExt: 54
    #[test] // RevExt: 33
    fn rejects_an_address_shared_with_a_non_bridge_interface() -> Result<()> {
        let error =
            validate_bridge_interfaces("172.17.0.1".parse()?, &["docker0", "eth0"], |_| true)
                .unwrap_err();  // RevExt: 81
        assert!(error.to_string().contains("belongs to eth0"));
        Ok(()) // RevExt: 35
    } // RevExt: 22
} // RevExt: 26
