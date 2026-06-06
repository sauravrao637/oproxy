# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
# Build
cargo build
cargo build --release

# Build the React UI assets required by Rust include_str! routes
corepack enable
yarn --cwd src/design install --frozen-lockfile
yarn --cwd src/design build

# Run all Rust tests with release warning policy
RUSTFLAGS="-D warnings" cargo test

# Run a single test by name
cargo test <test_name>

# Run tests in a specific module
cargo test middleware::plugins::jwt_inspector

# Lint
cargo clippy -- -D warnings

# Run the proxy. A clean checkout will build src/design/dist automatically
# if Node/Yarn are available; explicit UI build is still faster in CI.
cargo run
```

> **Critical:** run the full test suite before release, not only `cargo test --lib`. Browser tests live under `tests/browser` and use Playwright.

## Architecture

### Three-layer separation

1. **Transport** (`main.rs`, `core/engine.rs`) — hyper accept loop, CONNECT handling, MITM TLS, reqwest forwarding  
2. **Traffic manipulation** (`middleware/`) — inspect, rewrite, throttle, pause, mock  
3. **Control plane** (`management.rs`, `api/`, `storage.rs`) — axum REST API, web UI, JSON persistence

### Request lifecycle

```
hyper accept loop (main.rs)
  ├─ CONNECT request → mitm_intercept() or TCP tunnel (copy_bidirectional)
  └─ all other requests → proxy_dispatch_layer (axum middleware)
       ├─ Host == localhost → axum router (management UI / API)
       └─ else → ProxyEngine::handle_request()
            1. Buffer body (up to max_body_bytes)
            2. Run Request Middleware Chain (insertion order)
            3. Strip internal headers, resolve target URL
            4. Forward via reqwest
            5. Run Response Middleware Chain (reverse order)
            6. Return to client
```

### Middleware system

New traffic features = implement `Middleware` trait. No engine changes needed.

```rust
#[async_trait]
pub trait Middleware: Send + Sync {
    fn name(&self) -> &str;
    async fn on_request(&self, ctx: &mut RequestContext) -> MiddlewareAction;
    async fn on_response(&self, ctx: &mut ResponseContext) -> MiddlewareAction;
}
// MiddlewareAction: Continue | StopAndReturn | Pause
```

`StopAndReturn` returns 403 by default. To return a custom response (mock, Lua abort), embed a JSON payload in `ctx.headers["x-oproxy-mock-response"]` before returning `StopAndReturn`. The engine reads and serves it.

**Middleware chain insertion order** (`runtime/state.rs`):
1. AccessControl (block/allow rules)
2. CaptureFilter (skip recording for filtered hosts)
3. DnsOverride (rewrite upstream host)
4. MapRemote (routing table)
5. Throttling (latency/bandwidth)
6. Rewrite (request/response mutations)
7. Breakpoint (pause on match)
8. JwtInspector / GraphQLInspector / GrpcInspector (payload inspection)
9. Inspection (record session)
10. MapLocal (serve local files) ← short-circuits to file instead of forwarding
11. Mock (return canned responses)
12. Lua (custom request/response logic)
13. Inspection response-pass (record response)

Plugins running before step 9 record the original request. Steps 10–12 can short-circuit with `StopAndReturn`, preventing upstream forwarding.

### Internal middleware ↔ engine side-channel

Middleware plugins exchange data with each other and the engine through **typed,
in-memory fields** on `RequestContext` / `ResponseContext` (all `#[serde(skip)]`, so
they never serialise into recordings/exports and can never leak to the upstream
server). This replaces the former `x-oproxy-*` pseudo-header protocol — there is no
JSON-in-header encoding, no base64 round-trip for binary mock bodies, and a client
can no longer spoof these by sending matching headers (the engine defensively strips
any client-supplied `x-oproxy-*` request header before forwarding).

| Field | Set by | Read by | Purpose |
|---|---|---|---|
| `RequestContext.skip_recording: bool` | CaptureFilterMiddleware | InspectionMiddleware | Skip session recording for filtered hosts |
| `RequestContext.session_id: Option<String>` | InspectionMiddleware | engine.rs | Correlate response to exact request session |
| `RequestContext.destination: Option<String>` | RoutingMiddleware / DnsOverride / MITM | engine.rs | Override upstream target URL |
| `RequestContext.inspector: InspectorData` | JWT / GraphQL / gRPC inspectors | InspectionMiddleware | Parsed inspector data → `Exchange.inspector_data` |
| `RequestContext.mock_response: Option<InterceptedResponse>` | Mock / Rewrite / Lua abort / Breakpoint timeout | engine.rs on StopAndReturn | Short-circuit response returned instead of forwarding |

`InterceptedResponse { status: u16, headers, body: Bytes, tags: Vec<String> }` carries
the body as raw `Bytes`, so binary mock payloads survive without base64. The MITM TLS
layer passes its target via `engine.handle_request_with_destination(req, dest)` rather
than a header. `ResponseContext.tags: Vec<String>` carries session tags (e.g. `"mock"`)
to attach when the exchange is recorded.

### Binary body forwarding

`RequestContext.body` is a lossy UTF-8 string. `body_bytes` holds the original bytes. If a middleware modifies `body`, it **must** set `body_bytes = None`; otherwise the engine forwards the original bytes intact (critical for images, protobuf, zip).

### AppState

`Arc<AppState>` is shared by all axum handlers. Fields:

```
proxy_engine        Arc<ProxyEngine>             — reqwest clients + middleware chain
session_manager     Arc<SessionManager>          — in-memory traffic log + SSE broadcast
storage_path        PathBuf                      — JSON persistence directory
config              Config                       — startup config (immutable after init)
webhooks            Arc<RwLock<Vec<WebhookConfig>>>
mock_rules          Arc<RwLock<Vec<MockRule>>>
lua_scripts         Arc<RwLock<Vec<LuaScript>>>
breakpoint_manager  Arc<BreakpointManager>
api_handler         Arc<ApiHandler>              — session/rewrite/breakpoint CRUD
routing_table       Arc<RwLock<HashMap<...>>>
...                 (throttling, dns_overrides, map_local, capture_filter)
```

`ProxyEngine` uses `tokio::sync::RwLock<(Client, Client)>` internally for hot-reload of the upstream proxy config. Call `engine.http_client().await` to get a clone; call `engine.set_upstream_proxy(url).await` to rebuild clients.

### Persistence

`storage.rs` contains `load_*` / `save_*` pairs for each persisted type. The `save_*` functions are **async** and write via `tokio::fs` (atomic tmp-write + rename) so config persistence from axum handlers never blocks a Tokio worker thread. The `load_*` functions remain synchronous — they only run once at startup (`runtime/state.rs`), before the server accepts connections. Session data is **in-memory only** (lost on restart unless explicitly saved via `POST /admin/sessions/save`).

Storage files in `./storage/` (default):

```
routes.json, throttle.json, rewrites.json, breakpoints.json,
header_maps.json, modifications.json, hot_config.json,
capture_filter.json, dns_overrides.json, map_local.json,
upstream_proxy.json, webhooks.json, mock_rules.json, lua_scripts.json
```

### Configuration

Priority order: env vars > YAML (`OPROXY_CONFIG` → `./configs/default.yaml`) > defaults.

Key env vars: `OPROXY_PORT`, `OPROXY_BIND_HOST`, `OPROXY_MITM_ENABLED`, `OPROXY_STORAGE_PATH`, `OPROXY_LOG_LEVEL`, `RUST_LOG`, `OPROXY_MAP_LOCAL_BASE_PATH`.

`socks5_port` and `upstream_proxy` are config fields with no env var override — set via YAML or `POST /admin/upstream-proxy`.

`map_local_base_path` (via `OPROXY_MAP_LOCAL_BASE_PATH` env var) configures the base directory for Map Local fixture files in containerized deployments. When set, relative paths in rules are resolved from it; absolute paths still work for backward compatibility.

### Session data model

`Exchange` in `session/mod.rs` holds one captured request/response pair. Key fields:
- `request: RequestContext`, `response: Option<ResponseContext>`
- `metrics: Option<InspectionMetrics>` — latency, TTFB, body time, sizes; optional DNS/TCP/TLS breakdown
- `inspector_data: Option<InspectorData>` — JWT, GraphQL, gRPC parsed data (populated by inspector middlewares via InspectionMiddleware)
- `tags`, `note` — user annotations

`SessionManager` is an `RwLock<IndexMap<String, Exchange>>` with a cap-based eviction (oldest dropped when `max_sessions` is reached) and a `broadcast::Sender<()>` that fires on every change (SSE + webhook dispatcher).

### SOCKS5 listener

`transport/socks5.rs` implements RFC 1928 no-auth handshake. Integrated in `main.rs` when `config.socks5_port` is set — second `TcpListener` calls `transport::socks5::handshake()` then either `tunnel()` (plain TCP) or MITM path.

### Lua scripting

`middleware/plugins/lua_engine.rs` creates a fresh sandboxed `Lua` state per request (no shared state). Globals `io`, `os`, `package`, `require`, `load`, `loadfile`, `dofile`, `debug` are removed. Scripts interact via `request`/`response` table globals. `abort(status, body)` sets `x-oproxy-mock-response` and returns `StopAndReturn`. mlua uses `vendored` feature (bundles Lua 5.4 — no system Lua needed).

### Map Local (file mocking)

`middleware/plugins/map_local.rs` serves static fixture files for matching requests instead of forwarding upstream. Key details:

**Rule matching:** `MapLocalRule` uses full `Location`-based matching (host, path, method, protocol, query, mode). 

**Path resolution:**
- If `OPROXY_MAP_LOCAL_BASE_PATH` is set, relative paths are joined with it
- Absolute paths (starting with `/`) bypass the base path
- Backward compatible: existing deployments with absolute paths continue to work

**File vs directory:**
- **File mode:** serves the file's contents verbatim
- **Directory mode:** appends the request path and serves the result (with path-traversal checks)

**Validation:** 
- API layer (`control_plane/policy.rs`): `validate_map_local_path()` checks at rule creation time using the base path
- Returns 422 if file doesn't exist, with a helpful error message
- Runtime checks (middleware): 502 if the file/directory becomes inaccessible

**MIME detection:** `mime_for_path()` infers `Content-Type` from file extension (20+ types supported).

**Storage:** Rules persisted in `./storage/map_local_rules.json` by `storage.rs`.

**Docker pattern:** Set `OPROXY_MAP_LOCAL_BASE_PATH=/map-local` + `-v ./my-responses:/map-local`. Users then create rules with relative paths like `api/users.json`.

## UI

The current app shell is built from `src/design` with Vite. `management.rs` serves the built files from `src/design/dist` via `include_str!`, so clean Rust builds need those assets. `build.rs` generates them automatically when missing; Docker and GitHub workflows build the UI explicitly before compiling Rust.

The legacy static files under `src/index.html`, `src/app.css`, and `src/js/` are still present for older surfaces and compatibility, but `/` serves the built design app. The design app includes Sessions, Compose, Rules, Breakpoints, Mock, Lua, Inspectors, DNS, Capture Filter, Webhooks, Root CA, Map Local, and Settings surfaces.

The current app shell is built from `src/design` with Vite. `management.rs` serves the built files from `src/design/dist` via `include_str!`, so clean Rust builds need those assets. `build.rs` generates them automatically when missing; Docker and GitHub workflows build the UI explicitly before compiling Rust.

The legacy static files under `src/index.html`, `src/app.css`, and `src/js/` are still present for older surfaces and compatibility, but `/` serves the built design app. The design app includes Sessions, Compose, Rules, Breakpoints, Mock, Lua, Inspectors, DNS, Capture Filter, Webhooks, Root CA, and Settings surfaces.

## Web browsing & QA

For all web browsing, QA testing, and site dogfooding, use the `/browse` skill from **gstack** (installed at `~/.claude/skills/gstack`). Never use `mcp__claude-in-chrome__*` tools.

Available gstack skills:
- `/office-hours`, `/plan-ceo-review`, `/plan-eng-review`, `/plan-design-review`, `/design-consultation`, `/design-shotgun`, `/design-html`, `/review`, `/ship`, `/land-and-deploy`, `/canary`, `/benchmark`, `/browse`, `/connect-chrome`, `/qa`, `/qa-only`, `/design-review`, `/setup-browser-cookies`, `/setup-deploy`, `/setup-gbrain`, `/retro`, `/investigate`, `/document-release`, `/document-generate`, `/codex`, `/cso`, `/autoplan`, `/plan-devex-review`, `/devex-review`, `/careful`, `/freeze`, `/guard`, `/unfreeze`, `/gstack-upgrade`, `/learn`

## Skill routing

When the user's request matches an available skill, invoke it via the Skill tool. When in doubt, invoke the skill.

Key routing rules:
- Product ideas/brainstorming → invoke /office-hours
- Strategy/scope → invoke /plan-ceo-review
- Architecture → invoke /plan-eng-review
- Design system/plan review → invoke /design-consultation or /plan-design-review
- Full review pipeline → invoke /autoplan
- Bugs/errors → invoke /investigate
- QA/testing site behavior → invoke /qa or /qa-only
- Code review/diff check → invoke /review
- Visual polish → invoke /design-review
- Ship/deploy/PR → invoke /ship or /land-and-deploy
- Save progress → invoke /context-save
- Resume context → invoke /context-restore
- Author a backlog-ready spec/issue → invoke /spec
