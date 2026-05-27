use std::collections::HashMap;

pub const REDACTED: &str = "[redacted]";

pub fn is_sensitive_key(key: &str) -> bool {
    let key = key.trim().to_ascii_lowercase().replace('-', "_");
    matches!(
        key.as_str(),
        "authorization"
            | "cookie"
            | "set_cookie"
            | "x_api_key"
            | "api_key"
            | "access_token"
            | "refresh_token"
            | "password"
            | "secret"
            | "token"
    ) || key.contains("password")
        || key.contains("secret")
        || key.ends_with("_token")
        || key.contains("api_key")
}

pub fn redact_headers(headers: &HashMap<String, String>) -> HashMap<String, String> {
    headers
        .iter()
        .map(|(k, v)| {
            let value = if is_sensitive_key(k) {
                REDACTED.to_string()
            } else {
                v.clone()
            };
            (k.clone(), value)
        })
        .collect()
}

pub fn sensitive_values(headers: &HashMap<String, String>, body: &str) -> Vec<String> {
    let mut values = Vec::new();
    for (key, value) in headers {
        if is_sensitive_key(key) {
            push_sensitive_value(&mut values, value);
        }
    }
    collect_sensitive_body_values(body, &mut values);
    values.sort();
    values.dedup();
    values
}

pub fn redact_known_values(text: &str, values: &[String]) -> String {
    values.iter().fold(text.to_string(), |acc, value| {
        if should_redact_literal(value) {
            acc.replace(value, REDACTED)
        } else {
            acc
        }
    })
}

pub fn redact_body_text(body: &str) -> String {
    if body.trim().is_empty() {
        return body.to_string();
    }

    if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(body) {
        redact_json_value(&mut value);
        return serde_json::to_string_pretty(&value).unwrap_or_else(|_| REDACTED.to_string());
    }

    redact_urlencoded_like(body)
}

fn collect_sensitive_body_values(body: &str, out: &mut Vec<String>) {
    if body.trim().is_empty() {
        return;
    }

    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
        collect_sensitive_json_values(&value, "", out);
        return;
    }

    collect_sensitive_urlencoded_like_values(body, out);
}

fn collect_sensitive_json_values(value: &serde_json::Value, key: &str, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (child_key, child) in map {
                collect_sensitive_json_values(child, child_key, out);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_sensitive_json_values(item, key, out);
            }
        }
        serde_json::Value::String(s) if is_sensitive_key(key) => push_sensitive_value(out, s),
        serde_json::Value::Number(n) if is_sensitive_key(key) => {
            push_sensitive_value(out, &n.to_string())
        }
        serde_json::Value::Bool(b) if is_sensitive_key(key) => {
            push_sensitive_value(out, &b.to_string())
        }
        _ => {}
    }
}

fn collect_sensitive_urlencoded_like_values(body: &str, out: &mut Vec<String>) {
    let separator = if body.contains('&') { '&' } else { '\n' };
    for part in body.split(separator) {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        if is_sensitive_key(key) {
            push_sensitive_value(out, value);
        }
    }
}

fn push_sensitive_value(out: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if should_redact_literal(value) {
        out.push(value.to_string());
    }
}

fn should_redact_literal(value: &str) -> bool {
    value.len() >= 4 && value != REDACTED
}

fn redact_json_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if is_sensitive_key(key) {
                    *child = serde_json::Value::String(REDACTED.to_string());
                } else {
                    redact_json_value(child);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                redact_json_value(item);
            }
        }
        _ => {}
    }
}

fn redact_urlencoded_like(body: &str) -> String {
    let separator = if body.contains('&') { '&' } else { '\n' };
    body.split(separator)
        .map(|part| {
            let Some((key, value)) = part.split_once('=') else {
                return part.to_string();
            };
            if is_sensitive_key(key) {
                format!("{key}={REDACTED}")
            } else {
                format!("{key}={value}")
            }
        })
        .collect::<Vec<_>>()
        .join(&separator.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_sensitive_headers_case_insensitively() {
        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), "Bearer secret".to_string());
        headers.insert("content-type".to_string(), "application/json".to_string());

        let redacted = redact_headers(&headers);

        assert_eq!(
            redacted.get("Authorization").map(String::as_str),
            Some(REDACTED)
        );
        assert_eq!(
            redacted.get("content-type").map(String::as_str),
            Some("application/json")
        );
    }

    #[test]
    fn redacts_nested_json_body_fields() {
        let out = redact_body_text(r#"{"user":"a","auth":{"refresh_token":"rt","password":"pw"}}"#);

        assert!(out.contains(REDACTED));
        assert!(!out.contains("refresh_token\": \"rt"));
        assert!(!out.contains("password\": \"pw"));
    }

    #[test]
    fn redacts_form_fields() {
        let out = redact_body_text("api_key=abc&name=dev");

        assert_eq!(out, "api_key=[redacted]&name=dev");
    }

    #[test]
    fn collects_sensitive_values_for_reflected_response_redaction() {
        let mut headers = HashMap::new();
        headers.insert(
            "authorization".to_string(),
            "Bearer request-token".to_string(),
        );

        let values = sensitive_values(&headers, r#"{"token":"body-token","name":"dev"}"#);
        let redacted = redact_known_values(
            "echo Bearer request-token and body-token but keep dev",
            &values,
        );

        assert!(redacted.contains(REDACTED));
        assert!(!redacted.contains("request-token"));
        assert!(!redacted.contains("body-token"));
        assert!(redacted.contains("dev"));
    }
}
