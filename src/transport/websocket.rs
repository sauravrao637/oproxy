use std::collections::HashMap;
use std::pin::Pin;
use std::task::{Context, Poll};

use axum::body::Body;
use hyper::body::Incoming;
use hyper::{Request, Response};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::sync::watch;
use tokio::time::timeout;

use crate::transport::TransportContext;
use crate::transport::lifecycle::wait_for_shutdown;

pub fn is_websocket_upgrade<B>(req: &Request<B>) -> bool {
    req.headers()
        .get("upgrade")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false)
}

struct LeadingBytesReader<R> {
    prefix: std::io::Cursor<Vec<u8>>,
    inner: R,
}

impl<R: AsyncRead + Unpin> AsyncRead for LeadingBytesReader<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let me = &mut *self;
        let pos = me.prefix.position() as usize;
        let data = me.prefix.get_ref();
        if pos < data.len() {
            let to_read = (data.len() - pos).min(buf.remaining());
            buf.put_slice(&data[pos..pos + to_read]);
            me.prefix.set_position((pos + to_read) as u64);
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut me.inner).poll_read(cx, buf)
    }
}

async fn read_ws_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> std::io::Result<(u8, Vec<u8>, Vec<u8>)> {
    let mut header = [0u8; 2];
    reader.read_exact(&mut header).await?;
    let b0 = header[0];
    let b1 = header[1];
    let opcode = b0 & 0x0F;
    let masked = (b1 & 0x80) != 0;
    let len7 = (b1 & 0x7F) as usize;
    let mut raw = vec![b0, b1];
    let payload_len = match len7 {
        126 => {
            let mut ext = [0u8; 2];
            reader.read_exact(&mut ext).await?;
            raw.extend_from_slice(&ext);
            u16::from_be_bytes(ext) as usize
        }
        127 => {
            let mut ext = [0u8; 8];
            reader.read_exact(&mut ext).await?;
            raw.extend_from_slice(&ext);
            (u64::from_be_bytes(ext) as usize).min(16 * 1024 * 1024)
        }
        n => n,
    };
    let mask_key = if masked {
        let mut key = [0u8; 4];
        reader.read_exact(&mut key).await?;
        raw.extend_from_slice(&key);
        Some(key)
    } else {
        None
    };
    let mut payload = vec![0u8; payload_len];
    reader.read_exact(&mut payload).await?;
    raw.extend_from_slice(&payload);
    let decoded = if let Some(key) = mask_key {
        payload
            .iter()
            .enumerate()
            .map(|(i, &b)| b ^ key[i % 4])
            .collect()
    } else {
        payload
    };
    Ok((opcode, decoded, raw))
}

async fn relay_ws_frames<R, W>(
    mut reader: R,
    mut writer: W,
    sm: crate::session::SharedSessionManager,
    session_id: String,
    direction: crate::session::WsDirection,
) where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    loop {
        let (opcode, decoded, raw) = match read_ws_frame(&mut reader).await {
            Ok(f) => f,
            Err(_) => break,
        };
        if writer.write_all(&raw).await.is_err() {
            break;
        }
        let payload_len = decoded.len();
        let (payload_text, payload_hex) = if opcode == 0x1 {
            let s = String::from_utf8_lossy(&decoded[..decoded.len().min(512)]).into_owned();
            (Some(s), None)
        } else {
            let chunk = &decoded[..decoded.len().min(64)];
            let mut hex = String::with_capacity(chunk.len() * 2);
            for b in chunk {
                use std::fmt::Write as _;
                let _ = write!(hex, "{:02x}", b);
            }
            (None, if hex.is_empty() { None } else { Some(hex) })
        };
        sm.append_ws_frame(
            &session_id,
            crate::session::WsFrame {
                timestamp: chrono::Utc::now(),
                direction: direction.clone(),
                opcode,
                payload_len,
                payload_text,
                payload_hex,
            },
        );
        if opcode == 0x8 {
            break;
        }
    }
}

pub async fn handle_websocket(
    req: Request<Incoming>,
    context: TransportContext,
    session_id: String,
    peer: Option<std::net::SocketAddr>,
    mut shutdown: watch::Receiver<bool>,
) -> Response<Body> {
    let sm = context.session_manager.clone();
    let connections = context.connections.clone();
    let inspect_frames = context.inspect_ws_frames;
    let connect_timeout = context.connect_timeout;
    let handshake_timeout = context.handshake_timeout;

    let uri = req.uri().clone();
    let headers = req.headers().clone();

    let host_header = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let target_host = uri
        .host()
        .map(|s| s.to_string())
        .unwrap_or_else(|| host_header.split(':').next().unwrap_or("").to_string());
    let port: u16 = uri.port_u16().unwrap_or(80);
    let addr = format!("{}:{}", target_host, port);

    let path_and_query = uri
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());

    let mut raw_req = format!("GET {} HTTP/1.1\r\n", path_and_query);
    for (name, value) in &headers {
        if let Ok(v) = value.to_str() {
            raw_req.push_str(&format!("{}: {}\r\n", name, v));
        }
    }
    raw_req.push_str("\r\n");

    let mut upstream = match timeout(connect_timeout, tokio::net::TcpStream::connect(&addr)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            tracing::warn!(error=%e, addr=%addr, "WS upstream unreachable");
            return Response::builder()
                .status(502)
                .body(Body::from("WebSocket upstream unreachable"))
                .unwrap();
        }
        Err(_) => {
            tracing::warn!(addr=%addr, timeout_secs=connect_timeout.as_secs(), "WS upstream connect timed out");
            return Response::builder()
                .status(504)
                .body(Body::from("WebSocket upstream connect timed out"))
                .unwrap();
        }
    };

    let header_buf = match timeout(handshake_timeout, async {
        if let Err(e) = upstream.write_all(raw_req.as_bytes()).await {
            tracing::warn!(error=%e, "WS handshake send failed");
            return Err("WS handshake send failed");
        }

        let mut header_buf: Vec<u8> = Vec::with_capacity(1024);
        let mut tmp = [0u8; 512];
        'read: loop {
            match upstream.read(&mut tmp).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    header_buf.extend_from_slice(&tmp[..n]);
                    for i in 0..header_buf.len().saturating_sub(3) {
                        if &header_buf[i..i + 4] == b"\r\n\r\n" {
                            break 'read;
                        }
                    }
                    if header_buf.len() > 16_384 {
                        break;
                    }
                }
            }
        }
        Ok(header_buf)
    })
    .await
    {
        Ok(Ok(header_buf)) => header_buf,
        Ok(Err(message)) => {
            return Response::builder()
                .status(502)
                .body(Body::from(message))
                .unwrap();
        }
        Err(_) => {
            tracing::warn!(addr=%addr, timeout_secs=handshake_timeout.as_secs(), "WS handshake timed out");
            return Response::builder()
                .status(504)
                .body(Body::from("WebSocket handshake timed out"))
                .unwrap();
        }
    };

    let header_str = String::from_utf8_lossy(&header_buf);
    let first_line = header_str.lines().next().unwrap_or("");
    if !first_line.contains(" 101 ") {
        tracing::warn!(response=%first_line, addr=%addr, "WS upstream rejected upgrade");
        return Response::builder()
            .status(502)
            .body(Body::from("Upstream did not switch protocols"))
            .unwrap();
    }

    let mut builder = Response::builder().status(101);
    for line in header_str.lines().skip(1) {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(": ") {
            builder = builder.header(name, value);
        }
    }

    let mut req_headers_map = HashMap::new();
    for (k, v) in &headers {
        if let Ok(v) = v.to_str() {
            req_headers_map.insert(k.to_string(), v.to_string());
        }
    }
    sm.record_request(
        session_id.clone(),
        crate::middleware::RequestContext {
            method: "WS".to_string(),
            uri: format!("ws://{}:{}{}", target_host, port, path_and_query),
            headers: req_headers_map,
            body: String::new(),
            host: target_host.clone(),
            body_bytes: None,
        },
    );

    let header_end = header_buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| p + 4)
        .unwrap_or(header_buf.len());
    let leftover: Vec<u8> = header_buf[header_end..].to_vec();

    let on_upgrade = hyper::upgrade::on(req);
    connections.spawn_tracked("websocket-tunnel", peer, async move {
        let upgraded = tokio::select! {
            upgraded = on_upgrade => upgraded,
            _ = wait_for_shutdown(&mut shutdown) => {
                tracing::debug!("WS client upgrade stopped by shutdown");
                return;
            }
        };
        match upgraded {
            Ok(upgraded) => {
                let mut client_io = hyper_util::rt::TokioIo::new(upgraded);
                if !inspect_frames {
                    if !leftover.is_empty() {
                        let _ = client_io.write_all(&leftover).await;
                    }
                    tokio::select! {
                        result = tokio::io::copy_bidirectional(&mut client_io, &mut upstream) => {
                            if let Err(e) = result {
                                tracing::debug!(error=%e, "WS tunnel closed");
                            }
                        }
                        _ = wait_for_shutdown(&mut shutdown) => {
                            tracing::debug!("WS tunnel stopped by shutdown");
                        }
                    }
                    return;
                }
                let (client_read, client_write) = tokio::io::split(client_io);
                let (server_tcp_read, server_tcp_write) = upstream.into_split();
                let server_read = LeadingBytesReader {
                    prefix: std::io::Cursor::new(leftover),
                    inner: server_tcp_read,
                };
                let sm_c = sm.clone();
                let sid_c = session_id.clone();
                let relay_c = relay_ws_frames(
                    client_read,
                    server_tcp_write,
                    sm_c,
                    sid_c,
                    crate::session::WsDirection::ClientToServer,
                );
                let relay_s = relay_ws_frames(
                    server_read,
                    client_write,
                    sm,
                    session_id,
                    crate::session::WsDirection::ServerToClient,
                );
                tokio::pin!(relay_c);
                tokio::pin!(relay_s);
                tokio::select! {
                    _ = &mut relay_c => {}
                    _ = &mut relay_s => {}
                    _ = wait_for_shutdown(&mut shutdown) => {
                        tracing::debug!("WS frame relay stopped by shutdown");
                    }
                }
            }
            Err(e) => tracing::debug!(error=%e, "WS client upgrade failed"),
        }
    });

    builder
        .body(Body::empty())
        .unwrap_or_else(|_| Response::builder().status(500).body(Body::empty()).unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn websocket_upgrade_header_is_case_insensitive() {
        let req = Request::builder()
            .header("upgrade", "WebSocket")
            .body(())
            .unwrap();

        assert!(is_websocket_upgrade(&req));
    }
}
