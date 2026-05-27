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

/// Detect whether the process is running inside a container.
///
/// This is intentionally conservative. When running in Docker, the UDP socket
/// trick usually returns the container bridge address, which is not a reachable
/// LAN target for phones or other devices.
pub fn running_in_container() -> bool {
    std::path::Path::new("/.dockerenv").exists()
        || std::env::var("container").is_ok()
        || std::env::var("KUBERNETES_SERVICE_HOST").is_ok()
}

pub fn public_lan_ip_for_setup() -> Option<String> {
    let detected = detect_lan_ip()?;
    public_lan_ip_for_setup_from(Some(detected), running_in_container())
}

pub fn public_lan_ip_for_setup_from(
    detected: Option<String>,
    running_in_container: bool,
) -> Option<String> {
    if running_in_container { None } else { detected }
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

    #[test]
    fn public_lan_ip_for_setup_is_none_inside_detected_container() {
        if running_in_container() {
            assert!(
                public_lan_ip_for_setup().is_none(),
                "container bridge IPs must not be advertised as LAN setup targets"
            );
        }
    }

    #[test]
    fn public_lan_ip_for_setup_from_suppresses_container_ip() {
        assert_eq!(
            public_lan_ip_for_setup_from(Some("172.19.0.2".to_string()), true),
            None
        );
    }

    #[test]
    fn public_lan_ip_for_setup_from_keeps_host_ip_outside_container() {
        assert_eq!(
            public_lan_ip_for_setup_from(Some("192.168.1.20".to_string()), false),
            Some("192.168.1.20".to_string())
        );
    }
}
