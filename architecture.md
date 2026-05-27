# oproxy — Architecture

_Last updated: 2026-05-26. Reflects the current `dev` branch._

---

## Overview

oproxy is a programmable HTTP/HTTPS proxy. Its design follows a strict separation between three concerns:

1. **Runtime/transport** — assemble services, accept connections, handle CONNECT/SOCKS5/WebSocket tunnels, forward bytes (`runtime/`, `transport/`, `engine.rs`)
2. **Traffic manipulation** — inspect, rewrite, throttle, pause (`middleware/`)
3. **Control plane** — management UI, REST API, persistence (`control_plane.rs`, `control_plane/`, `api/`, `storage.rs`)

New traffic features are added by implementing the `Middleware` trait, without touching the engine.

---

## Component map

```
src/
├── main.rs           — thin Tokio entry point
├── control_plane.rs  — axum router: UI, admin API, proxy dispatch middleware
├── storage.rs        — JSON read/write helpers for persistent state
│
├── runtime/
│   ├── app.rs        — startup orchestration, service assembly, listener loops
│   ├── error.rs      — startup error model
│   ├── listeners.rs  — HTTP/HTTPS listener binding
│   ├── logging.rs    — tracing/log file setup
│   └── shutdown.rs   — ctrl-c/SIGTERM signal handling
│
├── control_plane/
│   ├── auth.rs       — admin auth, CSRF/origin checks, proxy dispatch
│   ├── sessions.rs   — session listing/export/import handlers
│   ├── policy.rs     — routes, throttling, rewrites, capture filter, DNS, map-local
│   ├── breakpoints.rs
│   ├── extensions.rs — plugins, playback, mocks, Lua scripts
│   ├── webhooks.rs
│   ├── settings.rs
│   ├── forward.rs
│   └── metrics.rs
│
├── core/
│   └── engine.rs     — ProxyEngine: HTTP request lifecycle and reqwest forwarding
│
├── transport/
│   ├── connect.rs    — HTTP CONNECT tunnel recording, DNS override, upstream dial
│   ├── http.rs       — shared HTTP/HTTPS connection service and upgrade dispatch
│   ├── lifecycle.rs  — connection limiting, shutdown watchers, graceful drain
│   ├── socks5.rs     — SOCKS5 handshake and TCP tunnel forwarding
│   ├── socks5_server.rs — SOCKS5 listener orchestration and MITM handoff
│   ├── tls.rs        — MITM TLS accept and per-host certificate serving
│   └── websocket.rs  — ws:// upgrade proxying and optional frame capture
│
├── middleware/
│   ├── mod.rs        — Middleware trait, MiddlewareAction enum, RequestContext/ResponseContext
│   ├── chain.rs      — MiddlewareChain: ordered execution, short-circuit on Stop/Pause
│   └── plugins/
│       ├── routing.rs      — host→destination remapping; artificial latency injection
│       ├── inspection.rs   — records every exchange into SessionManager
│       ├── rewrite.rs      — regex-based header/body rewrite rules
│       ├── modification.rs — static header injection/removal rules
│       └── breakpoints.rs  — pause-and-resume at request or response boundary
│
├── session/mod.rs    — SessionManager: in-memory HashMap, cap-based eviction, save/load
├── certs/mod.rs      — CertificateAuthority: root CA management, per-domain cert generation
├── config/mod.rs     — Config struct, YAML loading, env var overrides
└── api/mod.rs        — ApiHandler: session/rewrite/breakpoint CRUD used by the control plane
```

---

## Request lifecycle

```
Client
  │
  │  Plain HTTP or HTTPS CONNECT
  ▼
runtime::app — listener wiring and hyper accept loop
  │
  ├─ CONNECT? ──────────────────────────────────────────────────────────────────┐
  │                                                                             │
  │  plain HTTP/forward-proxy request                               mitm_enabled?
  ▼                                                                    │       │
proxy_dispatch_layer (axum middleware)                               yes      no
  │                                                                    │       │
  │  Host == localhost?                                       transport::tls  transport::connect
  ├─ no ──→ ProxyEngine::handle_request()                    (TLS accept,     (copy_bidirectional)
  │                                                           serve as HTTP)
  │  yes ──→ axum router (control-plane UI / API)
  │
  ▼
ProxyEngine::handle_request()
  │
  ├─ 1. Buffer request body (up to max_body_bytes)
  ├─ 2. Build RequestContext {method, uri, headers, body, host}
  ├─ 3. Run Request Middleware Chain ──────────────────────────────────────────┐
  │       RoutingMiddleware        sets x-proxy-destination header             │
  │       ThrottlingMiddleware     injects artificial latency                  │
  │       RewriteMiddleware        regex rewrite on headers/body               │
  │       BreakpointMiddleware     may Pause (blocks until UI resolves)        │
  │       InspectionMiddleware     opens session entry, injects session ID     │
  │       ModificationMiddleware   static header mutations                     │
  │                                                                            │
  │  MiddlewareAction::StopAndReturn → 403                                     │
  │  MiddlewareAction::Pause         → 202 (client waits)                      │
  │  MiddlewareAction::Continue      → forward ──────────────────────────────┘ │
  │                                                                            │
  ├─ 4. Strip internal headers (x-proxy-destination, x-oproxy-session-id,     │
  │       accept-encoding)                                                     │
  ├─ 5. Resolve target URL (route table or Host header passthrough)            │
  ├─ 6. Forward via reqwest (timeout from config, separate no-timeout client   │
  │       for SSE/event-stream responses)                                      │
  ├─ 7. Decompress gzip/deflate if upstream ignored stripped Accept-Encoding   │
  ├─ 8. Run Response Middleware Chain (same plugins, reverse order)            │
  └─ 9. Return response to client
```

---

## Middleware trait

```rust
#[async_trait]
pub trait Middleware: Send + Sync {
    fn name(&self) -> &str;
    async fn on_request(&self, ctx: &mut RequestContext) -> MiddlewareAction;
    async fn on_response(&self, ctx: &mut ResponseContext) -> MiddlewareAction;
}

pub enum MiddlewareAction {
    Continue,       // proceed to next middleware
    StopAndReturn,  // stop chain, return 403 to client
    Pause,          // block until externally resolved (breakpoints)
}
```

`MiddlewareChain` runs `on_request` in insertion order and `on_response` in reverse order, short-circuiting on the first non-`Continue` action.

---

## MITM / TLS interception

```
Client                    oproxy                      Upstream
  │                          │                            │
  │── CONNECT host:443 ──→   │                            │
  │←─ 200 Connection Est. ── │                            │
  │                          │── gen cert for host ──     │
  │── TLS ClientHello ──→    │   (signed by root CA)      │
  │←─ TLS ServerHello ───────│                            │
  │   (oproxy cert)          │                            │
  │                          │                            │
  │── HTTP request ─────→    │── HTTPS request ──────→   │
  │                          │←─ HTTPS response ──────── │
  │←─ HTTP response ─────────│                            │
```

The root CA (`certs/root.crt`) must be trusted by the client. Download it from `GET /admin/ca`. Domain certificates are generated on first use and cached in memory.

---

## Persistence

Runtime state is persisted to JSON files in `storage_path` (default `./storage/`):

| File | Content |
|---|---|
| `routes.json` | Routing table `{ "host": "destination" }` |
| `throttle.json` | ThrottlingConfig (enabled, latency_ms, per-host overrides) |
| `rewrites.json` | Array of RewriteRule |
| `breakpoints.json` | Array of BreakpointRule |

All files are written synchronously on mutation. The session log is in-memory only and is lost on restart.

---

## Configuration resolution order

Highest priority wins:

1. Environment variables (`OPROXY_PORT`, `OPROXY_BIND_HOST`, `OPROXY_MITM_ENABLED`, `OPROXY_STORAGE_PATH`, `OPROXY_LOG_LEVEL`, `OPROXY_LOG_DIR`, `OPROXY_MAX_CONNECTIONS`, `OPROXY_CONNECT_TIMEOUT_SECS`, `OPROXY_HANDSHAKE_TIMEOUT_SECS`, `OPROXY_SHUTDOWN_GRACE_SECS`, `OPROXY_ALLOW_REMOTE_ADMIN`, `OPROXY_ADMIN_TOKEN`, `OPROXY_ALLOW_PRIVATE_ADMIN_EGRESS`, `RUST_LOG`)
2. YAML config file (`OPROXY_CONFIG` env var → `./configs/default.yaml` → built-in defaults)
3. Built-in defaults

---

## Key design decisions

**Host-based proxy dispatch, not path-based**
A forward proxy receives absolute-URI requests (`GET http://api.example.com/ HTTP/1.1`). Axum's router would match `/` and serve the management UI for every proxied request. An axum middleware layer (`proxy_dispatch_layer`) inspects the `Host` header before route matching and short-circuits non-localhost requests directly to `ProxyEngine::handle_request`.
When the listener binds to `0.0.0.0`, remote management UI/API access is still disabled unless `allow_remote_admin` is enabled. This keeps LAN proxy exposure separate from LAN control-plane exposure.
Because this remains a single-port design and Host headers are client-controlled, remote management also requires `admin_token`. Localhost-style management hosts are accepted only for loopback downstream peers.

**Admin egress policy**
When remote management is enabled, admin-initiated outbound requests from `/admin/forward`, replay, and webhooks are blocked from private, loopback, link-local, multicast, and unspecified IP ranges unless `allow_private_admin_egress` is explicitly enabled. This keeps remote admin convenience from becoming a default SSRF primitive.

**Raw hyper accept loop**
CONNECT handling requires access to the raw TCP socket via `hyper::upgrade::on`. Axum's `with_upgrades()` severs the link between the 200-response task and the upgraded socket when routed through middleware. The solution is to bypass axum for upgrade traffic at the connection-service layer: `transport::http::ProxyHttpService` routes CONNECT to `transport::connect::handle_connect`, WebSocket upgrades to `transport::websocket::handle_websocket`, and ordinary requests to the axum app via `.oneshot()`.

**CA always initialised regardless of `mitm_enabled`**
`mitm_enabled` controls only whether CONNECT requests are intercepted. The CA is always started so `GET /admin/ca` works for certificate download even when MITM is off. Users can trust the cert in advance and flip the flag later without restarting.

**Session ID header for response correlation**
`InspectionMiddleware::on_request` injects `x-oproxy-session-id` into the request headers. The engine reads this value before forwarding and strips it from the upstream request. `on_response` uses the session ID for exact session lookup, avoiding correlation bugs under concurrent requests to the same URI.

**Binary body forwarding**
The middleware chain operates on a lossy UTF-8 string copy of the body. The engine keeps the original bytes separately. At forwarding time it compares the string copy against the original; if no middleware modified it, the original bytes are forwarded intact, preventing corruption of images, protobuf, zip, etc.

---

## Known limitations / planned work

| Area | Status |
|---|---|
| WebSocket proxying | **Implemented** — plain `ws://` proxied via TCP tunnel in `transport::websocket::handle_websocket()`; `wss://` works via CONNECT tunnel |
| Brotli decompression | **Implemented** — `Content-Encoding: br` decoded using `brotli` crate alongside gzip/deflate |
| Non-SSE response streaming | **Implemented** — responses with `Content-Length > 512 KB` use streaming path; smaller responses still buffered |
| Binary body in middleware | Partial — original bytes forwarded intact when no middleware modifies the body; if a rewrite rule edits the body, the binary is lossy-decoded then re-encoded as UTF-8, silently corrupting it |
| Async file I/O | **Implemented** — `save_to_file` / `load_from_file` use `tokio::fs` |
| Session pagination | **Implemented** — `GET /api/sessions?limit=N&offset=M&since=<timestamp>` |
| HTTPS listener | **Implemented** — `https_port` config field (or `OPROXY_HTTPS_PORT` env var); when set, a second TLS listener accepts HTTPS proxy connections; client must trust the CA |
| HTTP/2 downstream | Partial — listener uses hyper's auto builder, but HTTP/2 CONNECT, gRPC, and extended CONNECT behavior still need protocol compliance tests |
| Config hot reload | Config is read once at startup; changing the YAML file requires a restart (Low priority) |
| Metrics endpoint | **Implemented** — `GET /admin/metrics` returns aggregate latency/size stats |
| SSE polling | **Implemented** — `GET /api/sessions/stream` (SSE); UI subscribes once and refreshes on each event |
| Session save/load | **Implemented** — `POST /admin/sessions/save` and `POST /admin/sessions/load` |
| Playback | **Implemented** — `POST /admin/playback` re-issues all recorded requests via HTTP; replays appear in UI as `[REPLAY]` entries |
| Map local | **Implemented** — set `map_local` table in `RoutingMiddleware`; serves files from disk instead of forwarding |
| Bandwidth limiting | **Implemented** — `bandwidth_limit_kbps` in throttle config; simulates transfer time via proportional sleep on response body |
