use async_trait::async_trait;

use crate::middleware::{Middleware, MiddlewareAction, RequestContext, ResponseContext};
use crate::session::{GrpcField, GrpcInfo, GrpcMessage};

pub struct GrpcInspectorMiddleware;

impl GrpcInspectorMiddleware {
    pub fn is_grpc(ctx: &RequestContext) -> bool {
        ctx.headers
            .get("content-type")
            .map(|ct| ct.starts_with("application/grpc"))
            .unwrap_or(false)
    }

    /// Parse service and method from URI pattern `/package.ServiceName/MethodName`.
    pub fn parse_uri(uri: &str) -> (Option<String>, Option<String>) {
        let path = uri
            .trim_start_matches("http://")
            .trim_start_matches("https://");
        let path = if let Some(slash) = path.find('/') {
            &path[slash..]
        } else {
            path
        };
        let parts: Vec<&str> = path.trim_start_matches('/').splitn(2, '/').collect();
        if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
            (Some(parts[0].to_string()), Some(parts[1].to_string()))
        } else if parts.len() == 1 && !parts[0].is_empty() {
            (Some(parts[0].to_string()), None)
        } else {
            (None, None)
        }
    }

    /// Decode a gRPC framed message:
    /// [1 byte: compressed flag][4 bytes: big-endian message length][N bytes: protobuf]
    pub fn decode_grpc_frame(data: &[u8]) -> Option<(bool, Vec<u8>)> {
        if data.len() < 5 {
            return None;
        }
        let compressed = data[0] != 0;
        let msg_len = u32::from_be_bytes([data[1], data[2], data[3], data[4]]) as usize;
        if data.len() < 5 + msg_len {
            return None;
        }
        Some((compressed, data[5..5 + msg_len].to_vec()))
    }

    /// Wire-format decode without schema — extracts field_number, wire_type, value.
    pub fn decode_wire_format(data: &[u8]) -> Vec<GrpcField> {
        let mut fields = Vec::new();
        let mut pos = 0;

        while pos < data.len() {
            // Read varint tag
            let (tag, n) = match read_varint(data, pos) {
                Some(v) => v,
                None => break,
            };
            pos += n;
            let field_number = (tag >> 3) as u32;
            let wire_type = (tag & 0x7) as u8;

            let value = match wire_type {
                0 => {
                    // Varint
                    match read_varint(data, pos) {
                        Some((v, n)) => {
                            pos += n;
                            serde_json::Value::Number(v.into())
                        }
                        None => break,
                    }
                }
                1 => {
                    // 64-bit
                    if pos + 8 > data.len() {
                        break;
                    }
                    let bytes = &data[pos..pos + 8];
                    pos += 8;
                    let hex = bytes
                        .iter()
                        .map(|b| format!("{:02x}", b))
                        .collect::<String>();
                    serde_json::Value::String(hex)
                }
                2 => {
                    // Length-delimited
                    match read_varint(data, pos) {
                        Some((len, n)) => {
                            pos += n;
                            let len = len as usize;
                            if pos + len > data.len() {
                                break;
                            }
                            let bytes = &data[pos..pos + len];
                            pos += len;
                            // Try to decode as UTF-8 string, else hex
                            match std::str::from_utf8(bytes) {
                                Ok(s) => serde_json::Value::String(s.to_string()),
                                Err(_) => {
                                    let hex = bytes
                                        .iter()
                                        .map(|b| format!("{:02x}", b))
                                        .collect::<String>();
                                    serde_json::Value::String(hex)
                                }
                            }
                        }
                        None => break,
                    }
                }
                5 => {
                    // 32-bit
                    if pos + 4 > data.len() {
                        break;
                    }
                    let bytes = &data[pos..pos + 4];
                    pos += 4;
                    let hex = bytes
                        .iter()
                        .map(|b| format!("{:02x}", b))
                        .collect::<String>();
                    serde_json::Value::String(hex)
                }
                _ => {
                    // Unknown wire type — stop parsing
                    break;
                }
            };

            fields.push(GrpcField {
                field_number,
                wire_type,
                value,
            });
        }
        fields
    }
}

fn read_varint(data: &[u8], mut pos: usize) -> Option<(u64, usize)> {
    let mut result = 0u64;
    let mut shift = 0u32;
    let start = pos;
    loop {
        if pos >= data.len() || shift >= 64 {
            return None;
        }
        let byte = data[pos];
        pos += 1;
        result |= ((byte & 0x7f) as u64) << shift;
        shift += 7;
        if byte & 0x80 == 0 {
            break;
        }
    }
    Some((result, pos - start))
}

#[async_trait]
impl Middleware for GrpcInspectorMiddleware {
    fn name(&self) -> &str {
        "GrpcInspectorMiddleware"
    }

    async fn on_request(&self, ctx: &mut RequestContext) -> MiddlewareAction {
        if !Self::is_grpc(ctx) {
            return MiddlewareAction::Continue;
        }

        let (service, method) = Self::parse_uri(&ctx.uri);
        let body_bytes = ctx
            .body_bytes
            .as_ref()
            .map(|b| b.as_ref())
            .unwrap_or(ctx.body.as_bytes());

        let messages = if let Some((compressed, proto_bytes)) = Self::decode_grpc_frame(body_bytes)
        {
            let fields = Self::decode_wire_format(&proto_bytes);
            vec![GrpcMessage {
                direction: "request".to_string(),
                compressed,
                length: proto_bytes.len() as u32,
                fields,
            }]
        } else {
            vec![]
        };

        let info = GrpcInfo {
            service,
            method,
            messages,
        };
        if let Ok(json) = serde_json::to_string(&info) {
            ctx.headers.insert("x-oproxy-grpc".to_string(), json);
        }
        MiddlewareAction::Continue
    }

    async fn on_response(&self, _ctx: &mut ResponseContext) -> MiddlewareAction {
        MiddlewareAction::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_ctx(content_type: &str, uri: &str) -> RequestContext {
        let mut headers = HashMap::new();
        headers.insert("content-type".to_string(), content_type.to_string());
        RequestContext {
            method: "POST".to_string(),
            uri: uri.to_string(),
            headers,
            body: String::new(),
            host: "api.example.com".to_string(),
            body_bytes: None,
        }
    }

    #[test]
    fn grpc_content_type_detected() {
        let ctx = make_ctx("application/grpc", "/pkg.Service/Method");
        assert!(GrpcInspectorMiddleware::is_grpc(&ctx));
    }

    #[test]
    fn grpc_proto_content_type_detected() {
        let ctx = make_ctx("application/grpc+proto", "/pkg.Service/Method");
        assert!(GrpcInspectorMiddleware::is_grpc(&ctx));
    }

    #[test]
    fn non_grpc_not_detected() {
        let ctx = make_ctx("application/json", "/api");
        assert!(!GrpcInspectorMiddleware::is_grpc(&ctx));
    }

    #[test]
    fn service_and_method_extracted_from_uri() {
        let (svc, method) = GrpcInspectorMiddleware::parse_uri("/pkg.UserService/GetUser");
        assert_eq!(svc.as_deref(), Some("pkg.UserService"));
        assert_eq!(method.as_deref(), Some("GetUser"));
    }

    #[test]
    fn uri_with_host_prefix_parsed() {
        let (svc, method) =
            GrpcInspectorMiddleware::parse_uri("http://api.example.com/pkg.Service/Method");
        assert_eq!(svc.as_deref(), Some("pkg.Service"));
        assert_eq!(method.as_deref(), Some("Method"));
    }

    #[test]
    fn empty_uri_returns_none() {
        let (svc, method) = GrpcInspectorMiddleware::parse_uri("/");
        assert!(svc.is_none());
        assert!(method.is_none());
    }

    #[test]
    fn grpc_frame_parsed_correctly() {
        // Build a valid gRPC frame: [0x00][0x00 0x00 0x00 0x05][proto bytes]
        let proto = b"\x0a\x03foo"; // field 1, wire type 2, "foo"
        let mut frame = vec![0u8, 0, 0, 0, proto.len() as u8];
        frame.extend_from_slice(proto);
        let (compressed, data) = GrpcInspectorMiddleware::decode_grpc_frame(&frame).unwrap();
        assert!(!compressed);
        assert_eq!(data, proto);
    }

    #[test]
    fn compressed_frame_flag_set() {
        let proto = b"\x0a\x03foo";
        let mut frame = vec![0x01u8, 0, 0, 0, proto.len() as u8]; // compressed flag = 1
        frame.extend_from_slice(proto);
        let (compressed, _) = GrpcInspectorMiddleware::decode_grpc_frame(&frame).unwrap();
        assert!(compressed);
    }

    #[test]
    fn short_frame_returns_none() {
        assert!(GrpcInspectorMiddleware::decode_grpc_frame(&[0x00, 0x00]).is_none());
    }

    #[test]
    fn wire_format_varint_field_extracted() {
        // field 1, wire type 0 (varint), value 42
        // tag = (1 << 3) | 0 = 0x08, value = 42
        let data = vec![0x08u8, 0x2a];
        let fields = GrpcInspectorMiddleware::decode_wire_format(&data);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].field_number, 1);
        assert_eq!(fields[0].wire_type, 0);
        assert_eq!(fields[0].value, serde_json::json!(42));
    }

    #[test]
    fn wire_format_string_field_extracted() {
        // field 1, wire type 2 (length-delimited), value "hi"
        // tag = 0x0a, length = 2, "hi"
        let data = vec![0x0au8, 0x02, b'h', b'i'];
        let fields = GrpcInspectorMiddleware::decode_wire_format(&data);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].value, serde_json::json!("hi"));
    }

    #[test]
    fn empty_proto_gives_no_fields() {
        let fields = GrpcInspectorMiddleware::decode_wire_format(&[]);
        assert!(fields.is_empty());
    }
}
