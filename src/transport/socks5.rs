/// SOCKS5 proxy listener (RFC 1928).
///
/// Supports:
///   - No-auth method (0x00)
///   - CONNECT command only
///   - IPv4, IPv6, domain name address types
///
/// Integration with the proxy engine mirrors the existing CONNECT handler:
///   - TLS + MITM: calls `mitm_intercept()` (if `mitm_enabled`)
///   - Plain TCP: `tokio::io::copy_bidirectional`
use std::net::{Ipv4Addr, Ipv6Addr};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tracing::debug;

use std::sync::Arc;

use tokio::sync::{RwLock, watch};

use crate::core::engine::ProxyEngine;
use crate::middleware::plugins::dns_override::DnsOverrides;
use crate::middleware::plugins::mock::{MockBehavior, SharedMockRules, TunnelDecision};
use crate::transport::lifecycle::wait_for_shutdown;
use crate::transport::tls::{is_tls_port, mitm_intercept};

#[derive(Clone)]
pub struct ProxySocks5Service {
    pub engine: Arc<ProxyEngine>,
    pub dns: Arc<RwLock<DnsOverrides>>,
    pub mock_rules: SharedMockRules,
    pub connect_timeout: Duration,
    pub handshake_timeout: Duration,
}

impl ProxySocks5Service {
    pub async fn serve_connection(
        self,
        mut stream: tokio::net::TcpStream,
        mut shutdown: watch::Receiver<bool>,
    ) {
        let Some(resolved) = self.prepare_target(&mut stream).await else {
            return;
        };
        if !self.apply_tunnel_policy(&mut stream, &resolved).await {
            return;
        }
        if self.should_intercept(&resolved) {
            self.serve_mitm(stream, resolved).await;
            return;
        }
        self.serve_tunnel(stream, resolved, &mut shutdown).await;
    }

    async fn prepare_target(&self, stream: &mut TcpStream) -> Option<Socks5Target> {
        let target = match timeout(self.handshake_timeout, handshake(stream)).await {
            Ok(Ok(t)) => t,
            Ok(Err(e)) => {
                tracing::debug!(error=%e, "SOCKS5 handshake failed");
                return None;
            }
            Err(_) => {
                tracing::debug!("SOCKS5 handshake timed out");
                return None;
            }
        };
        Some(resolve_target(target, self.dns.clone()).await)
    }

    async fn apply_tunnel_policy(&self, stream: &mut TcpStream, target: &Socks5Target) -> bool {
        if let Some((rule_id, decision)) = self.tunnel_decision_for(target).await {
            if decision.delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(decision.delay_ms)).await;
            }
            self.engine
                .record_socks5_mock_served(
                    &target.host,
                    target.port,
                    rule_id,
                    "tunnel_decision".to_string(),
                )
                .await;
            if !decision.allow {
                let _ = send_failure_reply(stream, 0x02).await;
                return false;
            }
        }
        true
    }

    fn should_intercept(&self, target: &Socks5Target) -> bool {
        self.engine.mitm_enabled && is_tls_port(target.port) && self.engine.ca.is_some()
    }

    async fn serve_mitm(&self, mut stream: TcpStream, target: Socks5Target) {
        if let Some(ca) = self.engine.ca.clone() {
            if let Err(e) = timeout(self.handshake_timeout, send_success_reply(&mut stream))
                .await
                .unwrap_or(Err(Socks5Error::Io))
            {
                tracing::debug!(error=%e, "SOCKS5 success reply failed");
                return;
            }
            mitm_intercept(
                stream,
                target.host.clone(),
                format!("{}:{}", target.host, target.port),
                self.engine.clone(),
                ca,
                self.handshake_timeout,
            )
            .await;
        }
    }

    async fn serve_tunnel(
        &self,
        stream: TcpStream,
        target: Socks5Target,
        shutdown: &mut watch::Receiver<bool>,
    ) {
        let session_id = self
            .engine
            .record_socks5_tunnel_opened(&target.host, target.port)
            .await;

        let connection = tunnel_with_connect_timeout(stream, &target, self.connect_timeout);
        tokio::pin!(connection);
        tokio::select! {
            res = &mut connection => {
                let (bytes_up, bytes_down) = match res {
                    Ok(counts) => counts,
                    Err(e) => {
                        tracing::debug!(error=%e, "SOCKS5 tunnel error");
                        (0, 0)
                    }
                };
                if let Some(session_id) = session_id {
                    self.engine
                        .record_socks5_tunnel_closed(&session_id, bytes_up, bytes_down)
                        .await;
                }
            }
            _ = wait_for_shutdown(shutdown) => {
                tracing::debug!("SOCKS5 connection stopped by shutdown");
            }
        }
    }

    async fn tunnel_decision_for(&self, target: &Socks5Target) -> Option<(String, TunnelDecision)> {
        let ctx = socks5_request_context(target);
        let snapshots = {
            let rules = self.mock_rules.read().await;
            rules
                .iter()
                .filter(|rule| rule.enabled)
                .cloned()
                .collect::<Vec<_>>()
        };

        for rule in snapshots {
            let Some(MockBehavior::TunnelDecision { decision }) = rule.behavior.clone() else {
                continue;
            };
            if !rule.matches(&ctx) {
                continue;
            }
            // Look the live rule up by id, not snapshot index (rules may have
            // been edited/reordered concurrently).
            let mut rules = self.mock_rules.write().await;
            if let Some(live) = rules.iter_mut().find(|r| r.id == rule.id) {
                live.call_count += 1;
            }
            return Some((rule.id, decision));
        }
        None
    }
}

fn socks5_request_context(target: &Socks5Target) -> crate::middleware::RequestContext {
    crate::middleware::RequestContext {
        method: "CONNECT".to_string(),
        uri: format!("socks5://{}:{}", target.host, target.port),
        host: target.host.clone(),
        protocol_context: Some(crate::core::forward::ProtocolContext::socks5_tunnel()),
        downstream_protocol: Some(
            crate::core::forward::WireProtocol::Socks5
                .label()
                .to_string(),
        ),
        ..Default::default()
    }
}

async fn resolve_target(target: Socks5Target, dns: Arc<RwLock<DnsOverrides>>) -> Socks5Target {
    let resolved_host = {
        let overrides = dns.read().await;
        overrides
            .get(&target.host)
            .filter(|e| e.enabled)
            .map(|e| e.ip.clone())
            .unwrap_or_else(|| target.host.clone())
    };
    Socks5Target {
        host: resolved_host,
        port: target.port,
    }
}

/// SOCKS5 handshake + connect result.
#[derive(Debug)]
pub struct Socks5Target {
    pub host: String,
    pub port: u16,
}

/// Perform the SOCKS5 handshake and parse the CONNECT command.
/// Returns the target host:port on success.
pub async fn handshake(stream: &mut TcpStream) -> Result<Socks5Target, Socks5Error> {
    // ── Greeting ────────────────────────────────────────────────────────────
    // Client → Server: [0x05][n_methods][method1..methodN]
    let mut buf = [0u8; 2];
    stream
        .read_exact(&mut buf)
        .await
        .map_err(|_| Socks5Error::Io)?;
    let ver = buf[0];
    if ver != 5 {
        return Err(Socks5Error::BadVersion(ver));
    }
    let n_methods = buf[1] as usize;
    let mut methods = vec![0u8; n_methods];
    stream
        .read_exact(&mut methods)
        .await
        .map_err(|_| Socks5Error::Io)?;

    // We only support no-auth (0x00).
    if !methods.contains(&0x00) {
        // Server → Client: [0x05][0xFF] = no acceptable methods
        stream
            .write_all(&[0x05, 0xFF])
            .await
            .map_err(|_| Socks5Error::Io)?;
        return Err(Socks5Error::NoAcceptableMethod);
    }
    // Server → Client: [0x05][0x00] = no auth required
    stream
        .write_all(&[0x05, 0x00])
        .await
        .map_err(|_| Socks5Error::Io)?;

    // ── Request ─────────────────────────────────────────────────────────────
    // Client → Server: [0x05][cmd][0x00][addr_type][addr][port_hi][port_lo]
    let mut hdr = [0u8; 4];
    stream
        .read_exact(&mut hdr)
        .await
        .map_err(|_| Socks5Error::Io)?;
    if hdr[0] != 5 {
        return Err(Socks5Error::BadVersion(hdr[0]));
    }
    let cmd = hdr[1];
    if cmd != 0x01 {
        // Only CONNECT (0x01) supported; send COMMAND NOT SUPPORTED
        stream
            .write_all(&[0x05, 0x07, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
            .await
            .ok();
        return Err(Socks5Error::UnsupportedCommand(cmd));
    }
    let addr_type = hdr[3];

    let host = match addr_type {
        0x01 => {
            // IPv4
            let mut ip = [0u8; 4];
            stream
                .read_exact(&mut ip)
                .await
                .map_err(|_| Socks5Error::Io)?;
            Ipv4Addr::from(ip).to_string()
        }
        0x03 => {
            // Domain name
            let len = stream.read_u8().await.map_err(|_| Socks5Error::Io)? as usize;
            let mut name = vec![0u8; len];
            stream
                .read_exact(&mut name)
                .await
                .map_err(|_| Socks5Error::Io)?;
            String::from_utf8(name).map_err(|_| Socks5Error::InvalidAddress)?
        }
        0x04 => {
            // IPv6
            let mut ip = [0u8; 16];
            stream
                .read_exact(&mut ip)
                .await
                .map_err(|_| Socks5Error::Io)?;
            Ipv6Addr::from(ip).to_string()
        }
        _ => return Err(Socks5Error::UnsupportedAddrType(addr_type)),
    };

    let port_hi = stream.read_u8().await.map_err(|_| Socks5Error::Io)?;
    let port_lo = stream.read_u8().await.map_err(|_| Socks5Error::Io)?;
    let port = u16::from_be_bytes([port_hi, port_lo]);

    debug!("SOCKS5 CONNECT {} {}", host, port);

    Ok(Socks5Target { host, port })
}

#[derive(Debug, thiserror::Error)]
pub enum Socks5Error {
    #[error("I/O error during SOCKS5 handshake")]
    Io,
    #[error("unsupported SOCKS version: {0}")]
    BadVersion(u8),
    #[error("no acceptable authentication method")]
    NoAcceptableMethod,
    #[error("unsupported SOCKS5 command: {0:#04x}")]
    UnsupportedCommand(u8),
    #[error("unsupported address type: {0:#04x}")]
    UnsupportedAddrType(u8),
    #[error("invalid address encoding")]
    InvalidAddress,
    #[error("upstream connect failed: {0}")]
    ConnectFailed(String),
    #[error("upstream connect timed out")]
    ConnectTimeout,
}

fn connect_addr(target: &Socks5Target) -> String {
    if target.host.parse::<Ipv6Addr>().is_ok() {
        format!("[{}]:{}", target.host, target.port)
    } else {
        format!("{}:{}", target.host, target.port)
    }
}

pub async fn send_success_reply(stream: &mut TcpStream) -> Result<(), Socks5Error> {
    // Server → Client: success reply [0x05][0x00][0x00][0x01][0.0.0.0][0][0]
    stream
        .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
        .await
        .map_err(|_| Socks5Error::Io)
}

async fn send_failure_reply(stream: &mut TcpStream, code: u8) -> Result<(), Socks5Error> {
    stream
        .write_all(&[0x05, code, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
        .await
        .map_err(|_| Socks5Error::Io)
}

/// Forward a SOCKS5 stream with an explicit timeout for the upstream TCP dial.
pub async fn tunnel_with_connect_timeout(
    mut client: TcpStream,
    target: &Socks5Target,
    connect_timeout: Duration,
) -> Result<(u64, u64), Socks5Error> {
    let addr = connect_addr(target);
    let mut upstream = match timeout(connect_timeout, TcpStream::connect(&addr)).await {
        Ok(Ok(upstream)) => upstream,
        Ok(Err(e)) => {
            let _ = send_failure_reply(&mut client, 0x05).await;
            return Err(Socks5Error::ConnectFailed(e.to_string()));
        }
        Err(_) => {
            let _ = send_failure_reply(&mut client, 0x06).await;
            return Err(Socks5Error::ConnectTimeout);
        }
    };
    send_success_reply(&mut client).await?;
    let (bytes_up, bytes_down) = tokio::io::copy_bidirectional(&mut client, &mut upstream)
        .await
        .map_err(|_| Socks5Error::Io)?;
    Ok((bytes_up, bytes_down))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::engine::ProxyEngineConfig;
    use crate::middleware::matcher::{Location, MatchMode};
    use crate::middleware::plugins::dns_override::{DnsEntry, DnsOverrides};
    use crate::middleware::plugins::mock::MockRule;
    use std::collections::HashMap;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    fn test_engine() -> Arc<ProxyEngine> {
        Arc::new(ProxyEngine::new(ProxyEngineConfig {
            middleware_chain: Arc::new(RwLock::new(
                crate::middleware::chain::MiddlewareChain::new(),
            )),
            mitm_enabled: false,
            bind_host: "127.0.0.1".to_string(),
            ..Default::default()
        }))
    }

    fn test_service_with_mock_rules(rules: Vec<MockRule>) -> ProxySocks5Service {
        ProxySocks5Service {
            engine: test_engine(),
            dns: Arc::new(RwLock::new(DnsOverrides::new())),
            mock_rules: Arc::new(RwLock::new(rules)),
            connect_timeout: Duration::from_millis(50),
            handshake_timeout: Duration::from_millis(50),
        }
    }

    fn tunnel_rule(id: &str, host: &str, allow: bool, delay_ms: u64) -> MockRule {
        MockRule {
            id: id.to_string(),
            name: id.to_string(),
            enabled: true,
            location: Location {
                host: Some(host.to_string()),
                mode: MatchMode::Glob,
                wire_protocol: Some("socks5".to_string()),
                body_mode: Some("tunnel".to_string()),
                ..Default::default()
            },
            behavior: Some(MockBehavior::TunnelDecision {
                decision: TunnelDecision { allow, delay_ms },
            }),
            responses: Vec::new(),
            call_count: 0,
        }
    }

    #[tokio::test]
    async fn resolve_target_applies_dns_override_without_changing_port() {
        let dns = Arc::new(RwLock::new(DnsOverrides::from([(
            "example.test".to_string(),
            DnsEntry {
                ip: "127.0.0.1".to_string(),
                enabled: true,
            },
        )])));
        let resolved = resolve_target(
            Socks5Target {
                host: "example.test".to_string(),
                port: 8443,
            },
            dns,
        )
        .await;

        assert_eq!(resolved.host, "127.0.0.1");
        assert_eq!(resolved.port, 8443);
    }

    #[tokio::test]
    async fn tunnel_decision_matches_socks5_target_and_increments_count() {
        let service =
            test_service_with_mock_rules(vec![tunnel_rule("deny-api", "api.test", false, 25)]);

        let (rule_id, decision) = service
            .tunnel_decision_for(&Socks5Target {
                host: "api.test".to_string(),
                port: 443,
            })
            .await
            .expect("matching tunnel rule");

        assert_eq!(rule_id, "deny-api");
        assert!(!decision.allow);
        assert_eq!(decision.delay_ms, 25);
        assert_eq!(service.mock_rules.read().await[0].call_count, 1);
    }

    #[tokio::test]
    async fn tunnel_decision_ignores_legacy_http_mock_rules() {
        let mut rule = tunnel_rule("http-only", "api.test", false, 0);
        rule.behavior = None;
        rule.responses = vec![crate::middleware::plugins::mock::MockResponse {
            status: 200,
            headers: HashMap::new(),
            body: "nope".to_string(),
            delay_ms: 0,
        }];
        let service = test_service_with_mock_rules(vec![rule]);

        assert!(
            service
                .tunnel_decision_for(&Socks5Target {
                    host: "api.test".to_string(),
                    port: 443,
                })
                .await
                .is_none()
        );
    }

    /// Build a raw SOCKS5 no-auth greeting + IPv4 CONNECT request.
    fn build_connect_packet(ip: [u8; 4], port: u16) -> Vec<u8> {
        let mut pkt = vec![
            // Greeting: ver=5, 1 method, no-auth
            0x05, 0x01, 0x00, // Request: ver=5, CONNECT, RSV, IPv4
            0x05, 0x01, 0x00, 0x01,
        ];
        pkt.extend_from_slice(&ip);
        pkt.extend_from_slice(&port.to_be_bytes());
        pkt
    }

    /// Build a SOCKS5 packet with a domain address.
    fn build_domain_connect_packet(domain: &str, port: u16) -> Vec<u8> {
        let d = domain.as_bytes();
        let mut pkt = vec![
            0x05,
            0x01,
            0x00, // greeting
            0x05,
            0x01,
            0x00,
            0x03, // CONNECT, domain
            d.len() as u8,
        ];
        pkt.extend_from_slice(d);
        pkt.extend_from_slice(&port.to_be_bytes());
        pkt
    }

    fn build_ipv6_connect_packet(ip: [u8; 16], port: u16) -> Vec<u8> {
        let mut pkt = vec![
            0x05, 0x01, 0x00, // greeting
            0x05, 0x01, 0x00, 0x04, // CONNECT, IPv6
        ];
        pkt.extend_from_slice(&ip);
        pkt.extend_from_slice(&port.to_be_bytes());
        pkt
    }

    #[tokio::test]
    async fn no_auth_handshake_succeeds_ipv4() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let pkt = build_connect_packet([93, 184, 216, 34], 80);

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let target = handshake(&mut stream).await.unwrap();
            assert_eq!(target.host, "93.184.216.34");
            assert_eq!(target.port, 80);
        });

        let mut client = TcpStream::connect(addr).await.unwrap();
        client.write_all(&pkt).await.unwrap();
        // Read greeting reply. CONNECT success is sent only after upstream connect.
        let mut buf = [0u8; 2];
        client.read_exact(&mut buf).await.unwrap();
        assert_eq!(buf, [0x05, 0x00]);
    }

    #[tokio::test]
    async fn domain_address_parsed() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let pkt = build_domain_connect_packet("example.com", 443);

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let target = handshake(&mut stream).await.unwrap();
            assert_eq!(target.host, "example.com");
            assert_eq!(target.port, 443);
        });

        let mut client = TcpStream::connect(addr).await.unwrap();
        client.write_all(&pkt).await.unwrap();
        let mut buf = [0u8; 2];
        client.read_exact(&mut buf).await.unwrap();
        assert_eq!(buf, [0x05, 0x00]);
    }

    #[tokio::test]
    async fn bad_version_rejected() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let result = handshake(&mut stream).await;
            assert!(matches!(result, Err(Socks5Error::BadVersion(4))));
        });

        let mut client = TcpStream::connect(addr).await.unwrap();
        // Send SOCKS4 version
        client.write_all(&[0x04, 0x01, 0x00]).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    #[tokio::test]
    async fn no_acceptable_method_rejected() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let result = handshake(&mut stream).await;
            assert!(matches!(result, Err(Socks5Error::NoAcceptableMethod)));
        });

        let mut client = TcpStream::connect(addr).await.unwrap();
        // Offer only username/password auth (0x02), no no-auth
        client.write_all(&[0x05, 0x01, 0x02]).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    #[tokio::test]
    async fn unsupported_command_rejected() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let result = handshake(&mut stream).await;
            assert!(matches!(result, Err(Socks5Error::UnsupportedCommand(0x02))));
        });

        let mut client = TcpStream::connect(addr).await.unwrap();
        // Greeting with no-auth
        client.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
        // BIND command (0x02) instead of CONNECT
        client
            .write_all(&[0x05, 0x02, 0x00, 0x01, 127, 0, 0, 1, 0, 80])
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    #[tokio::test]
    async fn ipv6_address_parsed() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let target = handshake(&mut stream).await.unwrap();
            assert_eq!(target.host, "::1");
            assert_eq!(target.port, 8080);
        });

        let mut client = TcpStream::connect(addr).await.unwrap();
        let mut pkt = vec![0x05, 0x01, 0x00, 0x05, 0x01, 0x00, 0x04];
        // ::1 in 16 bytes
        let ipv6: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
        pkt.extend_from_slice(&ipv6);
        pkt.extend_from_slice(&8080u16.to_be_bytes());
        client.write_all(&pkt).await.unwrap();
        let mut buf = [0u8; 2];
        client.read_exact(&mut buf).await.unwrap();
        assert_eq!(buf, [0x05, 0x00]);
    }

    #[tokio::test]
    async fn ipv6_address_parsed_and_formatted_for_connect() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let pkt = build_ipv6_connect_packet(Ipv6Addr::LOCALHOST.octets(), 18291);

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let target = handshake(&mut stream).await.unwrap();
            assert_eq!(target.host, "::1");
            assert_eq!(target.port, 18291);
            assert_eq!(connect_addr(&target), "[::1]:18291");
        });

        let mut client = TcpStream::connect(addr).await.unwrap();
        client.write_all(&pkt).await.unwrap();
        let mut buf = [0u8; 2];
        client.read_exact(&mut buf).await.unwrap();
        assert_eq!(buf, [0x05, 0x00]);
    }

    #[tokio::test]
    async fn tunnel_sends_failure_reply_when_upstream_connect_fails() {
        let unused_upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let unused_port = unused_upstream.local_addr().unwrap().port();
        drop(unused_upstream);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let target = Socks5Target {
                host: "127.0.0.1".to_string(),
                port: unused_port,
            };
            let result =
                tunnel_with_connect_timeout(stream, &target, Duration::from_secs(30)).await;
            assert!(matches!(result, Err(Socks5Error::ConnectFailed(_))));
        });

        let mut client = TcpStream::connect(addr).await.unwrap();
        let mut reply = [0u8; 10];
        client.read_exact(&mut reply).await.unwrap();
        assert_eq!(reply[0], 0x05);
        assert_eq!(reply[1], 0x05);
    }
}
