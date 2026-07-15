use std::net::UdpSocket;

/// Detect the LAN IP of this machine using the UDP socket trick.
/// Opens a UDP socket "connecting" to 8.8.8.8:80 (no packets sent),
/// then reads the local address the OS assigned — which is the LAN IP.
/// Returns `None` if the machine has no network interface.
///
/// Works correctly outside Docker and inside host-networked Docker containers
/// (the default `network_mode: host`). With bridge networking the returned IP
/// is the container bridge address, which is unreachable from the LAN — but
/// bridge networking requires manual port-mapping config anyway, so the QR
/// cannot be made to work automatically in that case regardless.
pub fn public_lan_ip_for_setup() -> Option<String> {
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.connect("8.8.8.8:80")?;
            s.local_addr()
        })
        .ok()
        .map(|a| a.ip().to_string())
}

/// Resolves the host/IP the setup wizard should advertise (network-info JSON,
/// CA-download QR code), preferring an explicit operator override over
/// auto-detection.
///
/// `public_lan_ip_for_setup()`'s UDP-socket trick reports whatever address
/// *this* machine would use to reach the outside world - inside a container
/// that's the container's own bridge/internal IP (e.g. `172.17.0.2`), which
/// is unreachable from the Docker host or a real LAN client.
/// There's no way to detect the *actual* host-reachable address from inside
/// the container, so this can only be fixed with an explicit operator
/// override (`OPROXY_ADVERTISED_HOST` / `Config.advertised_host`) - falling
/// back to auto-detection when unset preserves today's behavior for native
/// (non-containerized) and host-networked deployments, where the detected
/// address is already correct.
pub fn advertised_lan_ip(advertised_host: Option<&str>) -> Option<String> {
    advertised_host
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .map(str::to_string)
        .or_else(public_lan_ip_for_setup)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_lan_ip_for_setup_returns_valid_address_or_none() {
        if let Some(ip) = public_lan_ip_for_setup() {
            assert!(
                ip.parse::<std::net::IpAddr>().is_ok(),
                "must return a valid IP address, got: {ip}"
            );
        }
        // None is acceptable in isolated CI environments with no network.
    }

    #[test]
    fn public_lan_ip_for_setup_is_not_loopback_when_present() {
        if let Some(ip) = public_lan_ip_for_setup() {
            assert!(
                !ip.starts_with("127."),
                "LAN IP must not be loopback, got: {ip}"
            );
            assert!(ip != "::1", "LAN IP must not be IPv6 loopback");
        }
    }

    #[test]
    fn advertised_lan_ip_prefers_explicit_override_over_auto_detection() {
        assert_eq!(
            advertised_lan_ip(Some("192.168.1.50")),
            Some("192.168.1.50".to_string()),
            "an explicit override must win, bypassing auto-detection entirely"
        );
    }

    #[test]
    fn advertised_lan_ip_trims_and_ignores_blank_override() {
        assert_eq!(
            advertised_lan_ip(Some("  10.0.0.5  ")),
            Some("10.0.0.5".to_string()),
            "surrounding whitespace from an env var must not leak into the advertised address"
        );
        // A blank/whitespace-only override must fall back to auto-detection
        // rather than advertising an empty host.
        assert_eq!(advertised_lan_ip(Some("")), public_lan_ip_for_setup());
        assert_eq!(advertised_lan_ip(Some("   ")), public_lan_ip_for_setup());
    }

    #[test]
    fn advertised_lan_ip_falls_back_to_auto_detection_when_unset() {
        assert_eq!(
            advertised_lan_ip(None),
            public_lan_ip_for_setup(),
            "unset override must preserve today's auto-detection behavior"
        );
    }
}
