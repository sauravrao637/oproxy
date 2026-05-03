use crate::session::Exchange;
use crate::middleware::{RequestContext, ResponseContext};

// ── Export ────────────────────────────────────────────────────────────────────

pub fn export_as_curl(ex: &Exchange) -> String {
    let req = &ex.request;
    let mut parts: Vec<String> = vec![format!("curl -X {} '{}'", req.method, req.uri)];

    for (k, v) in &req.headers {
        let kl = k.to_lowercase();
        if kl == "content-length" || kl == "transfer-encoding" {
            continue;
        }
        let escaped = v.replace('\'', "'\\''");
        parts.push(format!("-H '{}: {}'", k, escaped));
    }

    if !req.body.is_empty() {
        let escaped = req.body.replace('\'', "'\\''");
        parts.push(format!("--data '{}'", escaped));
    }

    parts.join(" \\\n  ")
}

pub fn export_as_fetch(ex: &Exchange) -> String {
    let req = &ex.request;
    let method = req.method.as_str();

    let headers_obj: Vec<String> = req.headers.iter()
        .filter(|(k, _)| {
            let kl = k.to_lowercase();
            kl != "content-length" && kl != "transfer-encoding"
        })
        .map(|(k, v)| format!("    \"{}\": \"{}\"", k, v.replace('"', "\\\"")))
        .collect();

    let mut opts_parts = vec![
        format!("  method: \"{}\"", method),
        format!("  headers: {{\n{}\n  }}", headers_obj.join(",\n")),
    ];

    if !req.body.is_empty() {
        opts_parts.push(format!("  body: \"{}\"", req.body.replace('"', "\\\"")));
    }

    format!(
        "fetch(\"{}\", {{\n{}\n}});",
        req.uri,
        opts_parts.join(",\n")
    )
}

pub fn export_as_python(ex: &Exchange) -> String {
    let req = &ex.request;

    let headers_entries: Vec<String> = req.headers.iter()
        .filter(|(k, _)| {
            let kl = k.to_lowercase();
            kl != "content-length" && kl != "transfer-encoding"
        })
        .map(|(k, v)| format!("    \"{}\": \"{}\"", k, v.replace('"', "\\\"")))
        .collect();

    let headers_str = format!("{{\n{}\n}}", headers_entries.join(",\n"));

    let body_line = if req.body.is_empty() {
        String::new()
    } else {
        let escaped = req.body.replace('"', "\\\"").replace('\n', "\\n");
        format!("\ndata = \"{}\"\n", escaped)
    };

    let data_arg = if req.body.is_empty() { "" } else { ", data=data" };

    format!(
        "import requests\n\nurl = \"{}\"\nheaders = {}{}\nresponse = requests.request(\"{}\", url, headers=headers{})\nprint(response.text)",
        req.uri, headers_str, body_line, req.method, data_arg
    )
}

// ── Import (parse curl command) ───────────────────────────────────────────────

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct ParsedCurl {
    pub method: String,
    pub url: String,
    pub headers: std::collections::HashMap<String, String>,
    pub body: String,
}

/// Parse a `curl` command string into a `ParsedCurl`.
/// Handles: `-X METHOD`, `-H 'Name: Value'`, `--data`/`-d`/`--data-raw`, URL.
/// Strips shell line continuations (`\` + newline) before parsing.
pub fn parse_curl(input: &str) -> Result<ParsedCurl, String> {
    // Normalise: strip leading "curl", collapse line continuations, tokenise.
    let normalised = input
        .trim()
        .trim_start_matches("curl")
        .replace("\\\n", " ")
        .replace("\\\r\n", " ");

    let tokens = tokenise_shell(&normalised)?;
    let mut out = ParsedCurl {
        method: String::new(),
        url: String::new(),
        headers: std::collections::HashMap::new(),
        body: String::new(),
    };
    let mut i = 0usize;
    while i < tokens.len() {
        match tokens[i].as_str() {
            "-X" | "--request" => {
                i += 1;
                if i < tokens.len() {
                    out.method = tokens[i].clone();
                }
            }
            "-H" | "--header" => {
                i += 1;
                if i < tokens.len() {
                    if let Some((k, v)) = tokens[i].split_once(':') {
                        out.headers.insert(k.trim().to_string(), v.trim().to_string());
                    }
                }
            }
            "-d" | "--data" | "--data-raw" | "--data-binary" | "--data-ascii" => {
                i += 1;
                if i < tokens.len() {
                    // Strip single @ (file reference prefix) if present
                    let val = tokens[i].trim_start_matches('@');
                    out.body = val.to_string();
                }
            }
            "-G" | "--get" => {
                out.method = "GET".to_string();
            }
            "-I" | "--head" => {
                out.method = "HEAD".to_string();
            }
            t if !t.starts_with('-') && out.url.is_empty() => {
                out.url = t.to_string();
            }
            _ => {}
        }
        i += 1;
    }

    if out.url.is_empty() {
        return Err("No URL found in curl command".to_string());
    }
    if out.method.is_empty() {
        out.method = if out.body.is_empty() { "GET" } else { "POST" }.to_string();
    }
    Ok(out)
}

/// Minimal POSIX-style shell tokeniser: handles single-quoted, double-quoted,
/// and unquoted tokens separated by whitespace. No variable expansion.
fn tokenise_shell(s: &str) -> Result<Vec<String>, String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut chars = s.chars().peekable();
    loop {
        // skip whitespace
        while chars.peek().map(|c| c.is_whitespace()).unwrap_or(false) {
            chars.next();
        }
        match chars.peek() {
            None => break,
            Some('\'') => {
                chars.next();
                let mut tok = String::new();
                loop {
                    match chars.next() {
                        None => return Err("Unterminated single-quoted string".to_string()),
                        Some('\'') => break,
                        Some(c) => tok.push(c),
                    }
                }
                tokens.push(tok);
            }
            Some('"') => {
                chars.next();
                let mut tok = String::new();
                loop {
                    match chars.next() {
                        None => return Err("Unterminated double-quoted string".to_string()),
                        Some('"') => break,
                        Some('\\') => {
                            if let Some(next) = chars.next() {
                                tok.push(match next {
                                    '"' | '\\' | 'n' | 't' => match next {
                                        'n' => '\n',
                                        't' => '\t',
                                        c => c,
                                    },
                                    c => c,
                                });
                            }
                        }
                        Some(c) => tok.push(c),
                    }
                }
                tokens.push(tok);
            }
            _ => {
                let mut tok = String::new();
                loop {
                    match chars.peek() {
                        None | Some(' ') | Some('\t') => break,
                        _ => tok.push(chars.next().unwrap()),
                    }
                }
                tokens.push(tok);
            }
        }
    }
    Ok(tokens)
}

// ── Convert ParsedCurl → RequestContext ──────────────────────────────────────

impl From<ParsedCurl> for RequestContext {
    fn from(p: ParsedCurl) -> Self {
        let host = extract_host(&p.url);
        RequestContext {
            method: p.method,
            uri: p.url,
            headers: p.headers,
            body: p.body.clone(),
            host,
            body_bytes: if p.body.is_empty() { None } else { Some(bytes::Bytes::from(p.body.into_bytes())) },
        }
    }
}

fn extract_host(url: &str) -> String {
    // Strip scheme then take up to the first / or end.
    let rest = if let Some(r) = url.find("://").map(|i| &url[i + 3..]) { r } else { url };
    rest.split('/').next().unwrap_or("").to_string()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Exchange;
    use crate::middleware::{RequestContext, ResponseContext};
    use chrono::Utc;

    fn make_exchange(method: &str, uri: &str, headers: Vec<(&str, &str)>, body: &str) -> Exchange {
        let mut hmap = std::collections::HashMap::new();
        for (k, v) in headers { hmap.insert(k.to_string(), v.to_string()); }
        Exchange {
            id: "test-id".to_string(),
            timestamp: Utc::now(),
            updated_at: None,
            request: RequestContext {
                method: method.to_string(),
                uri: uri.to_string(),
                headers: hmap,
                body: body.to_string(),
                host: "example.com".to_string(),
                body_bytes: None,
            },
            response: None,
            metrics: None,
            ws_frames: vec![],
            note: None,
            tags: vec![],
            inspector_data: None,
        }
    }

    // ── export_as_curl ────────────────────────────────────────────────────────

    #[test]
    fn curl_get_no_body() {
        let ex = make_exchange("GET", "https://example.com/api", vec![("accept", "application/json")], "");
        let out = export_as_curl(&ex);
        assert!(out.contains("curl -X GET 'https://example.com/api'"));
        assert!(out.contains("-H 'accept: application/json'"));
        assert!(!out.contains("--data"));
    }

    #[test]
    fn curl_post_with_body() {
        let ex = make_exchange("POST", "https://example.com/api", vec![("content-type", "application/json")], r#"{"key":"val"}"#);
        let out = export_as_curl(&ex);
        assert!(out.contains("curl -X POST"));
        assert!(out.contains("--data"));
        assert!(out.contains("key"));
    }

    #[test]
    fn curl_strips_content_length_header() {
        let ex = make_exchange("POST", "https://example.com/", vec![("content-length", "42"), ("x-custom", "yes")], "body");
        let out = export_as_curl(&ex);
        assert!(!out.contains("content-length"));
        assert!(out.contains("x-custom"));
    }

    #[test]
    fn curl_escapes_single_quotes_in_header() {
        let ex = make_exchange("GET", "https://x.com/", vec![("x-val", "it's here")], "");
        let out = export_as_curl(&ex);
        assert!(out.contains("it'\\''s here"));
    }

    // ── export_as_fetch ───────────────────────────────────────────────────────

    #[test]
    fn fetch_get_no_body() {
        let ex = make_exchange("GET", "https://example.com/", vec![], "");
        let out = export_as_fetch(&ex);
        assert!(out.contains("fetch(\"https://example.com/\""));
        assert!(out.contains("\"GET\""));
        assert!(!out.contains("body:"));
    }

    #[test]
    fn fetch_post_with_body() {
        let ex = make_exchange("POST", "https://example.com/", vec![], "hello");
        let out = export_as_fetch(&ex);
        assert!(out.contains("\"POST\""));
        assert!(out.contains("body:"));
    }

    // ── export_as_python ──────────────────────────────────────────────────────

    #[test]
    fn python_get_contains_requests_import() {
        let ex = make_exchange("GET", "https://example.com/", vec![], "");
        let out = export_as_python(&ex);
        assert!(out.contains("import requests"));
        assert!(out.contains("requests.request(\"GET\""));
        assert!(!out.contains("data=data"));
    }

    #[test]
    fn python_post_includes_data() {
        let ex = make_exchange("POST", "https://example.com/", vec![], "payload");
        let out = export_as_python(&ex);
        assert!(out.contains("data = "));
        assert!(out.contains("data=data"));
    }

    // ── parse_curl ────────────────────────────────────────────────────────────

    #[test]
    fn parse_simple_get() {
        let c = parse_curl("curl https://example.com/api").unwrap();
        assert_eq!(c.method, "GET");
        assert_eq!(c.url, "https://example.com/api");
        assert!(c.body.is_empty());
    }

    #[test]
    fn parse_explicit_method() {
        let c = parse_curl("curl -X DELETE https://example.com/item/1").unwrap();
        assert_eq!(c.method, "DELETE");
        assert_eq!(c.url, "https://example.com/item/1");
    }

    #[test]
    fn parse_post_with_data() {
        let c = parse_curl(r#"curl -X POST https://api.example.com/create -H 'Content-Type: application/json' -d '{"name":"test"}'"#).unwrap();
        assert_eq!(c.method, "POST");
        assert_eq!(c.url, "https://api.example.com/create");
        assert_eq!(c.headers.get("Content-Type").map(String::as_str), Some("application/json"));
        assert!(c.body.contains("name"));
    }

    #[test]
    fn parse_multiline_curl() {
        let cmd = "curl -X POST \\\n  https://example.com/ \\\n  -H 'x-key: abc'";
        let c = parse_curl(cmd).unwrap();
        assert_eq!(c.method, "POST");
        assert_eq!(c.url, "https://example.com/");
        assert_eq!(c.headers.get("x-key").map(String::as_str), Some("abc"));
    }

    #[test]
    fn parse_infers_post_when_body_present() {
        let c = parse_curl(r#"curl https://x.com/ -d 'data'"#).unwrap();
        assert_eq!(c.method, "POST");
        assert_eq!(c.body, "data");
    }

    #[test]
    fn parse_infers_head_method() {
        let c = parse_curl("curl -I https://example.com/").unwrap();
        assert_eq!(c.method, "HEAD");
    }

    #[test]
    fn parse_missing_url_returns_error() {
        assert!(parse_curl("curl -X GET -H 'foo: bar'").is_err());
    }

    #[test]
    fn parse_multiple_headers() {
        let c = parse_curl(r#"curl https://x.com -H 'Accept: */*' -H 'Authorization: Bearer tok'"#).unwrap();
        assert_eq!(c.headers.get("Accept").map(String::as_str), Some("*/*"));
        assert_eq!(c.headers.get("Authorization").map(String::as_str), Some("Bearer tok"));
    }

    #[test]
    fn tokenise_respects_quoted_whitespace() {
        let tokens = tokenise_shell(r#"-H 'X-Foo: a b c'"#).unwrap();
        assert_eq!(tokens, vec!["-H", "X-Foo: a b c"]);
    }

    // ── From<ParsedCurl> for RequestContext ───────────────────────────────────

    #[test]
    fn from_parsed_curl_extracts_host() {
        let p = ParsedCurl {
            method: "GET".to_string(),
            url: "https://api.example.com/v1/items".to_string(),
            headers: std::collections::HashMap::new(),
            body: String::new(),
        };
        let ctx: RequestContext = p.into();
        assert_eq!(ctx.host, "api.example.com");
        assert_eq!(ctx.method, "GET");
    }
}
