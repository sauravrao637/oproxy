# oproxy

A programmable HTTP/HTTPS proxy for inspecting, debugging, and manipulating network traffic. Written in Rust.

![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)

---

## What it does

- **Traffic inspection** — every proxied request and response is captured in a live, searchable session log
- **HTTPS MITM** — intercepts HTTPS by acting as a local CA; generates per-domain certificates on the fly
- **Routing** — redirect traffic from one host to another without touching the client
- **Rewrites** — regex-based header and body manipulation on request or response
- **Breakpoints** — pause a request or response mid-flight, inspect it, modify it, then continue or drop it
- **Throttling** — inject artificial latency per host to simulate slow networks
- **Management UI** — built-in web dashboard, no external dependencies; installable as a PWA

---

## Quick start

**Prerequisites:** Rust toolchain (`rustup`, `cargo`)

```bash
git clone https://github.com/sauravrao637/oproxy.git
cd oproxy
cargo run --release
```

The proxy starts on `http://0.0.0.0:8080`.  
Open `http://localhost:8080` in a browser to access the management dashboard.

### Configure your client

Point your HTTP proxy to `localhost:8080`.

**curl:**
```bash
curl -x http://localhost:8080 http://example.com/api/data
```

**Browser (e.g. Firefox):** Settings → Network → Manual proxy → HTTP `localhost:8080`

### HTTPS interception (MITM)

1. Enable MITM in `configs/default.yaml`:
   ```yaml
   mitm:
     enabled: true
   ```
2. Start oproxy, then download and install the root CA:
   ```bash
   curl http://localhost:8080/admin/ca -o oproxy-ca.crt
   ```
   Trust this certificate in your OS/browser certificate store.
3. Configure your client to use `localhost:8080` as the HTTPS proxy.

> The CA key and certificate are generated automatically on first run and stored in `./certs/`. **Never commit `certs/root.key` to version control.**

---

## Configuration

Copy and edit `configs/default.yaml`. The config file path can be overridden with `OPROXY_CONFIG`.

```yaml
port: 8080
bind_host: "0.0.0.0"       # use "127.0.0.1" to restrict to localhost

mitm:
  enabled: false
  root_ca_path: ./certs

storage_path: ./storage     # persisted routes, rewrites, breakpoints

timeout_secs: 30
max_body_bytes: 10485760    # 10 MB — bodies larger than this are truncated
max_sessions: 10000         # oldest session evicted when full

pool_max_idle_per_host: 10
pool_idle_timeout_secs: 30

log:
  level: info               # trace | debug | info | warn | error
  dir: .
  file: server.log
```

**Environment variable overrides** (highest priority):

| Variable | Description |
|---|---|
| `OPROXY_PORT` | Listening port |
| `OPROXY_BIND_HOST` | Bind address |
| `OPROXY_MITM_ENABLED` | `true` / `false` |
| `OPROXY_STORAGE_PATH` | Storage directory |
| `OPROXY_LOG_LEVEL` | Log level |
| `OPROXY_LOG_DIR` | Log file directory |
| `OPROXY_CONFIG` | Path to config file |
| `RUST_LOG` | Fine-grained tracing filter (overrides `log.level`) |

---

## Management API

All endpoints are served on `localhost` only. Requests from other hosts bypass the management layer and are treated as proxy traffic.

### Sessions

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/sessions` | List all captured sessions (supports `?since=<ISO8601>` for polling) |
| `GET` | `/api/sessions/:id` | Full detail for one session |
| `DELETE` | `/admin/sessions` | Clear all sessions |

### Routing

| Method | Path | Description |
|---|---|---|
| `GET` | `/admin/routes` | List routing rules |
| `POST` | `/admin/routes` | Replace routing table (JSON object `{ "host": "destination" }`) |

### Rewrites

| Method | Path | Description |
|---|---|---|
| `GET` | `/admin/rewrites` | List rewrite rules |
| `POST` | `/admin/rewrites` | Add a rewrite rule |
| `DELETE` | `/admin/rewrites/:index` | Delete a rule by index |

### Throttling

| Method | Path | Description |
|---|---|---|
| `GET` | `/admin/throttling` | Get throttling config |
| `POST` | `/admin/throttling` | Update throttling config |

### Breakpoints

| Method | Path | Description |
|---|---|---|
| `GET` | `/admin/breakpoints` | List breakpoint rules |
| `POST` | `/admin/breakpoints` | Add a breakpoint rule |
| `DELETE` | `/admin/breakpoints/:id` | Delete a breakpoint rule |
| `GET` | `/admin/breakpoints/pending` | List requests currently paused at a breakpoint |
| `POST` | `/admin/breakpoints/pending/:id/resolve` | Resolve a paused request (`continue`, `modify`, or `drop`) |

### Other

| Method | Path | Description |
|---|---|---|
| `GET` | `/health` | Health check — returns uptime and MITM status |
| `GET` | `/admin/ca` | Download the root CA certificate (PEM) |

---

## Project structure

```
oproxy/
├── src/
│   ├── main.rs              # Entry point — wires components, starts listener
│   ├── management.rs        # Axum router — UI, admin API, proxy dispatch layer
│   ├── lib.rs               # Crate root for integration tests
│   ├── index.html           # Management UI (single file, inlined at compile time)
│   ├── manifest.json        # PWA manifest
│   ├── sw.js                # Service worker (offline shell caching)
│   ├── icon.svg             # App icon
│   ├── storage.rs           # JSON persistence helpers
│   ├── api/                 # ApiHandler — session CRUD, rewrite CRUD, breakpoints
│   ├── certs/               # CertificateAuthority — root CA + per-domain cert gen
│   ├── config/              # Config struct, YAML loading, env var overrides
│   ├── core/
│   │   ├── engine.rs        # ProxyEngine — request lifecycle, CONNECT/MITM, forwarding
│   │   └── playback.rs      # Session replay scaffold
│   ├── middleware/
│   │   ├── chain.rs         # MiddlewareChain — ordered plugin execution
│   │   └── plugins/
│   │       ├── routing.rs   # RoutingMiddleware, ThrottlingMiddleware
│   │       ├── inspection.rs# InspectionMiddleware — session recording
│   │       ├── rewrite.rs   # RewriteMiddleware — regex rules
│   │       ├── modification.rs
│   │       └── breakpoints.rs
│   └── session/             # SessionManager — in-memory traffic log
├── tests/                   # Integration tests
├── configs/
│   └── default.yaml         # Default configuration
├── Dockerfile
└── Cargo.toml
```

---

## Docker

```bash
docker build -t oproxy .
docker run -p 8080:8080 oproxy
```

---

## Contributing

1. Fork the repo and create a feature branch from `main`.
2. Run `cargo test` — all tests must pass.
3. Run `cargo clippy -- -D warnings` — no new warnings.
4. Open a pull request with a clear description of what changed and why.

---

## License

MIT — see [LICENSE](LICENSE).
