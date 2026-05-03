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
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::{debug, warn};

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
    stream.read_exact(&mut buf).await.map_err(|_| Socks5Error::Io)?;
    let ver = buf[0];
    if ver != 5 {
        return Err(Socks5Error::BadVersion(ver));
    }
    let n_methods = buf[1] as usize;
    let mut methods = vec![0u8; n_methods];
    stream.read_exact(&mut methods).await.map_err(|_| Socks5Error::Io)?;

    // We only support no-auth (0x00).
    if !methods.contains(&0x00) {
        // Server → Client: [0x05][0xFF] = no acceptable methods
        stream.write_all(&[0x05, 0xFF]).await.map_err(|_| Socks5Error::Io)?;
        return Err(Socks5Error::NoAcceptableMethod);
    }
    // Server → Client: [0x05][0x00] = no auth required
    stream.write_all(&[0x05, 0x00]).await.map_err(|_| Socks5Error::Io)?;

    // ── Request ─────────────────────────────────────────────────────────────
    // Client → Server: [0x05][cmd][0x00][addr_type][addr][port_hi][port_lo]
    let mut hdr = [0u8; 4];
    stream.read_exact(&mut hdr).await.map_err(|_| Socks5Error::Io)?;
    if hdr[0] != 5 {
        return Err(Socks5Error::BadVersion(hdr[0]));
    }
    let cmd = hdr[1];
    if cmd != 0x01 {
        // Only CONNECT (0x01) supported; send COMMAND NOT SUPPORTED
        stream.write_all(&[0x05, 0x07, 0x00, 0x01, 0, 0, 0, 0, 0, 0]).await.ok();
        return Err(Socks5Error::UnsupportedCommand(cmd));
    }
    let addr_type = hdr[3];

    let host = match addr_type {
        0x01 => {
            // IPv4
            let mut ip = [0u8; 4];
            stream.read_exact(&mut ip).await.map_err(|_| Socks5Error::Io)?;
            Ipv4Addr::from(ip).to_string()
        }
        0x03 => {
            // Domain name
            let len = stream.read_u8().await.map_err(|_| Socks5Error::Io)? as usize;
            let mut name = vec![0u8; len];
            stream.read_exact(&mut name).await.map_err(|_| Socks5Error::Io)?;
            String::from_utf8(name).map_err(|_| Socks5Error::InvalidAddress)?
        }
        0x04 => {
            // IPv6
            let mut ip = [0u8; 16];
            stream.read_exact(&mut ip).await.map_err(|_| Socks5Error::Io)?;
            Ipv6Addr::from(ip).to_string()
        }
        _ => return Err(Socks5Error::UnsupportedAddrType(addr_type)),
    };

    let port_hi = stream.read_u8().await.map_err(|_| Socks5Error::Io)?;
    let port_lo = stream.read_u8().await.map_err(|_| Socks5Error::Io)?;
    let port = u16::from_be_bytes([port_hi, port_lo]);

    debug!("SOCKS5 CONNECT {} {}", host, port);

    // Server → Client: success reply [0x05][0x00][0x00][0x01][0.0.0.0][0][0]
    stream.write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0]).await.map_err(|_| Socks5Error::Io)?;

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
}

/// Forward a SOCKS5 stream (after successful handshake) as a plain TCP tunnel.
/// Used when MITM is disabled or the target is not TLS.
pub async fn tunnel(mut client: TcpStream, target: &Socks5Target) -> Result<(), Socks5Error> {
    let addr = format!("{}:{}", target.host, target.port);
    let mut upstream = TcpStream::connect(&addr).await
        .map_err(|e| Socks5Error::ConnectFailed(e.to_string()))?;
    tokio::io::copy_bidirectional(&mut client, &mut upstream)
        .await
        .map_err(|_| Socks5Error::Io)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    /// Build a raw SOCKS5 no-auth greeting + IPv4 CONNECT request.
    fn build_connect_packet(ip: [u8; 4], port: u16) -> Vec<u8> {
        let mut pkt = vec![
            // Greeting: ver=5, 1 method, no-auth
            0x05, 0x01, 0x00,
            // Request: ver=5, CONNECT, RSV, IPv4
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
            0x05, 0x01, 0x00,           // greeting
            0x05, 0x01, 0x00, 0x03,     // CONNECT, domain
            d.len() as u8,
        ];
        pkt.extend_from_slice(d);
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
        // Read greeting reply + connect reply
        let mut buf = [0u8; 12];
        tokio::io::AsyncReadExt::read(&mut client, &mut buf).await.ok();
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
        let mut buf = [0u8; 12];
        tokio::io::AsyncReadExt::read(&mut client, &mut buf).await.ok();
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
        client.write_all(&[0x05, 0x02, 0x00, 0x01, 127, 0, 0, 1, 0, 80]).await.unwrap();
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
        let ipv6: [u8; 16] = [0,0,0,0, 0,0,0,0, 0,0,0,0, 0,0,0,1];
        pkt.extend_from_slice(&ipv6);
        pkt.extend_from_slice(&8080u16.to_be_bytes());
        client.write_all(&pkt).await.unwrap();
        let mut buf = [0u8; 12];
        tokio::io::AsyncReadExt::read(&mut client, &mut buf).await.ok();
    }
}
