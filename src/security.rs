use std::net::IpAddr;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Default)]
pub struct AdminEgressPolicy {
    pub block_private_targets: bool,
}

impl AdminEgressPolicy {
    pub fn from_config(config: &crate::config::Config) -> Self {
        Self {
            block_private_targets: config.allow_remote_admin && !config.allow_private_admin_egress,
        }
    }
}

pub async fn enforce_admin_egress_policy(
    url: &reqwest::Url,
    policy: AdminEgressPolicy,
) -> Result<(), String> {
    if !policy.block_private_targets {
        return Ok(());
    }

    let Some(host) = url.host_str() else {
        return Err("admin egress URL must include a host".to_string());
    };

    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_protected_admin_egress_ip(ip) {
            return Err(format!(
                "admin egress to protected address {ip} is blocked while remote admin is enabled"
            ));
        }
        return Ok(());
    }

    let Some(port) = url.port_or_known_default() else {
        return Err("admin egress URL must include a resolvable port".to_string());
    };

    let resolved = tokio::time::timeout(
        Duration::from_secs(2),
        tokio::net::lookup_host((host, port)),
    )
    .await;
    let addrs = match resolved {
        Ok(Ok(addrs)) => addrs,
        Ok(Err(_)) | Err(_) => return Ok(()),
    };
    for addr in addrs {
        let ip = addr.ip();
        if is_protected_admin_egress_ip(ip) {
            return Err(format!(
                "admin egress target {host} resolved to protected address {ip}"
            ));
        }
    }
    Ok(())
}

pub fn is_protected_admin_egress_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let octets = ip.octets();
            ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_multicast()
                || ip.is_broadcast()
                || ip.is_unspecified()
                || (octets[0] == 100 && (64..=127).contains(&octets[1]))
        }
        IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip.is_multicast()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn protected_admin_egress_ip_classification_covers_local_ranges() {
        assert!(is_protected_admin_egress_ip(IpAddr::V4(
            Ipv4Addr::LOCALHOST
        )));
        assert!(is_protected_admin_egress_ip(IpAddr::V6(
            Ipv6Addr::LOCALHOST
        )));
        assert!(is_protected_admin_egress_ip(IpAddr::V4(Ipv4Addr::new(
            169, 254, 169, 254
        ))));
        assert!(is_protected_admin_egress_ip(IpAddr::V4(Ipv4Addr::new(
            10, 0, 0, 1
        ))));
        assert!(!is_protected_admin_egress_ip(IpAddr::V4(Ipv4Addr::new(
            93, 184, 216, 34
        ))));
    }

    #[tokio::test]
    async fn remote_admin_egress_policy_blocks_loopback_without_explicit_opt_in() {
        let policy = AdminEgressPolicy {
            block_private_targets: true,
        };
        let url = reqwest::Url::parse("http://127.0.0.1:8080/admin").unwrap();

        assert!(enforce_admin_egress_policy(&url, policy).await.is_err());

        let local_only_policy = AdminEgressPolicy {
            block_private_targets: false,
        };
        assert!(
            enforce_admin_egress_policy(&url, local_only_policy)
                .await
                .is_ok()
        );
    }
}
