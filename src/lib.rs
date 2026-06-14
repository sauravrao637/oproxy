//! # oproxy
//!
//! Library crate behind the `oproxy` binary: an intercepting HTTP/HTTPS proxy
//! with a middleware pipeline, MITM TLS, and a web control plane.
//!
//! ## What this crate exposes
//!
//! The modules below are the supported surface that integration tests (under
//! `tests/`) and embedders build against. The rough layering is:
//!
//! - [`core`] — the proxy engine: accept/forward loop, streaming, decompression.
//! - [`middleware`] — the [`middleware::Middleware`] trait and built-in plugins
//!   (rewrite, mock, breakpoints, inspectors, map-local, …).
//! - [`transport`] — connection handling: CONNECT/MITM, WebSocket, HTTP/3, SOCKS5.
//! - [`session`], [`storage`], [`har`], [`export`], [`diff`], [`webhooks`] —
//!   capture, persistence, and traffic export.
//! - [`config`], [`certs`], [`security`], [`redaction`], [`setup`],
//!   [`telemetry`] — startup configuration and cross-cutting concerns.
//! - [`api`] — REST handlers shared with the binary's control plane.
//!
//! Note: the `oproxy` **binary** additionally compiles `control_plane` and
//! `runtime` modules (web UI, REST router, process supervision) that are not
//! part of this library's public API; they live only in the binary target.

pub mod api;
pub mod certs;
pub mod config;
pub mod core;
pub mod diff;
pub mod examples;
pub mod export;
pub mod har;
pub mod middleware;
pub mod redaction;
pub mod security;
pub mod session;
pub mod setup;
pub mod storage;
pub mod telemetry;
pub mod transport;
pub mod webhooks;
