//! Free-text and field-scoped search over recorded [`Exchange`]s.
//!
//! `tag:`, `host:`, `method:`, and `status:` prefixes select a field; any
//! other token is matched as case-insensitive free text across the request
//! and response. See [`parse_search_query`] for the grammar.

use super::Exchange;

pub enum SearchTerm {
    Tag(String),
    Host(String),
    Method(String),
    Status(u16),
    Text(String),
}

impl SearchTerm {
    pub fn matches(&self, ex: &Exchange) -> bool {
        match self {
            SearchTerm::Tag(t) => ex
                .tags
                .iter()
                .any(|tag| tag.to_lowercase().contains(t.as_str())),
            SearchTerm::Host(h) => ex.request.host.to_lowercase().contains(h.as_str()),
            SearchTerm::Method(m) => ex.request.method.to_lowercase() == m.as_str(),
            SearchTerm::Status(s) => ex
                .response
                .as_ref()
                .map(|r| r.status == *s)
                .unwrap_or(false),
            SearchTerm::Text(t) => {
                let t = t.as_str();
                ex.request.uri.to_lowercase().contains(t)
                    || ex.request.body_text().to_lowercase().contains(t)
                    || ex
                        .request
                        .headers
                        .iter()
                        .any(|(k, v)| k.to_lowercase().contains(t) || v.to_lowercase().contains(t))
                    || ex
                        .response
                        .as_ref()
                        .map(|r| {
                            r.body_text().to_lowercase().contains(t)
                                || r.headers.iter().any(|(k, v)| {
                                    k.to_lowercase().contains(t) || v.to_lowercase().contains(t)
                                })
                        })
                        .unwrap_or(false)
                    || ex
                        .note
                        .as_deref()
                        .map(|n| n.to_lowercase().contains(t))
                        .unwrap_or(false)
            }
        }
    }
}

pub fn parse_search_query(query: &str) -> Vec<SearchTerm> {
    query
        .split_whitespace()
        .filter(|s| !s.is_empty())
        .map(|token| {
            if let Some(t) = token.strip_prefix("tag:") {
                SearchTerm::Tag(t.to_lowercase())
            } else if let Some(h) = token.strip_prefix("host:") {
                SearchTerm::Host(h.to_lowercase())
            } else if let Some(m) = token.strip_prefix("method:") {
                SearchTerm::Method(m.to_lowercase())
            } else if let Some(s) = token.strip_prefix("status:") {
                s.parse::<u16>()
                    .map(SearchTerm::Status)
                    .unwrap_or_else(|_| SearchTerm::Text(s.to_lowercase()))
            } else {
                SearchTerm::Text(token.to_lowercase())
            }
        })
        .collect()
}
