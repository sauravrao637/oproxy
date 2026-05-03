use std::net::UdpSocket;

/// Detect the LAN IP of this machine using the UDP socket trick.
/// Opens a UDP socket "connecting" to 8.8.8.8:80 (no packets sent),
/// then reads the local address the OS assigned — which is the LAN IP.
/// Returns `None` if the machine has no network interface.
pub fn detect_lan_ip() -> Option<String> {
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.connect("8.8.8.8:80")?;
            s.local_addr()
        })
        .ok()
        .map(|a| a.ip().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_lan_ip_returns_valid_address_or_none() {
        if let Some(ip) = detect_lan_ip() {
            assert!(
                ip.parse::<std::net::IpAddr>().is_ok(),
                "detect_lan_ip must return a valid IP address, got: {ip}"
            );
        }
        // None is acceptable in isolated CI environments with no network.
    }

    #[test]
    fn detect_lan_ip_is_not_loopback_when_present() {
        if let Some(ip) = detect_lan_ip() {
            assert!(
                !ip.starts_with("127."),
                "LAN IP must not be loopback, got: {ip}"
            );
            assert!(ip != "::1", "LAN IP must not be IPv6 loopback");
        }
    }
}
