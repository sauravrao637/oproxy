# oproxy

oproxy is a local developer proxy for inspecting, replaying, and modifying HTTP and HTTPS traffic, with a SOCKS5 listener for tunnel forwarding.

It is meant to run on your machine or in a local Docker container while you debug browsers, CLIs, mobile apps, API clients, services, and test suites. It is not a hosted SaaS product. There is no telemetry service, and captured traffic stays in the local storage directory or Docker volume you choose.

## What You Can Do

- Capture HTTP and HTTPS traffic with request/response headers, bodies, status, timing, and inspector data.
- Intercept HTTPS by trusting oproxy's locally generated root CA.
- Use the same listener as a browser/CLI HTTP proxy and a local management UI.
- Replay captured sessions and open captured requests in Compose.
- Craft requests in Compose, add headers/params/body, send them, save them to collections, and export cURL.
- Modify traffic with routes, DNS overrides, header maps, rewrites, request modifications, map-local files, throttling, mock responses, breakpoints, Lua scripts, capture filters, and webhooks.
- Export sessions as HAR/cURL with sensitive fields redacted by default.
- Run with Docker using persistent volumes for CA material and local state.

## Quick Start

### Docker

```bash
docker run --rm \
  --name oproxy \
  -p 127.0.0.1:8080:8080 \
  -p 127.0.0.1:1080:1080 \
  -e OPROXY_BIND_HOST=0.0.0.0 \
  -e OPROXY_MITM_ENABLED=true \
  -v oproxy-certs:/app/certs \
  -v oproxy-storage:/app/storage \
  ghcr.io/sauravrao637/oproxy:latest
```

Open `http://127.0.0.1:8080`.

Inside Docker, oproxy binds to `0.0.0.0` so Docker can forward traffic into the container. The command above publishes ports only on host loopback, so other machines cannot reach the admin UI/API unless you intentionally change the port mapping.

### Docker Compose

```bash
docker compose up --build
```

The included `docker-compose.yml` publishes:

- `127.0.0.1:8080:8080` for the HTTP proxy and UI
- `127.0.0.1:1080:1080` for SOCKS5
- `oproxy-certs` for the generated CA
- `oproxy-storage` for rules and persisted state

### From Source

Requirements:

- Rust 1.85 or newer
- Node.js 22 or newer

```bash
git clone https://github.com/sauravrao637/oproxy.git
cd oproxy
npm install --prefix src/design
npm run build --prefix src/design
cargo run --release
```

Open `http://127.0.0.1:8080`.

Rust serves the built UI from `src/design/dist`. If the UI is blank or stale while developing, rebuild it with `npm run build --prefix src/design`.

## Connecting Clients

Point your client at oproxy, then make requests normally.

### curl

HTTP:

```bash
curl -x http://127.0.0.1:8080 http://example.com
```

HTTPS with MITM after trusting or passing the oproxy CA:

```bash
curl http://127.0.0.1:8080/admin/ca -o oproxy-ca.crt
curl --cacert oproxy-ca.crt -x http://127.0.0.1:8080 https://example.com
```

SOCKS5:

```bash
curl --socks5-hostname 127.0.0.1:1080 http://example.com
```

The SOCKS5 listener is tunnel-only in the current beta. Use the HTTP proxy listener when you need requests to appear in the Sessions workbench.

### Browsers

Configure a manual proxy:

- HTTP proxy: `127.0.0.1`, port `8080`
- HTTPS proxy: `127.0.0.1`, port `8080`
- SOCKS host, optional: `127.0.0.1`, port `1080`

To inspect HTTPS, install the CA from `http://127.0.0.1:8080/admin/ca` into the browser or operating system trust store. Some browsers use a separate trust store.

### Node, Python, Go, and Other CLI Tools

Most HTTP clients respect these environment variables:

```bash
export HTTP_PROXY=http://127.0.0.1:8080
export HTTPS_PROXY=http://127.0.0.1:8080
export NO_PROXY=localhost,127.0.0.1
```

For HTTPS inspection, also configure the runtime to trust the downloaded oproxy CA. The exact variable or flag depends on the tool; for example, many Node tools use `NODE_EXTRA_CA_CERTS=/path/to/oproxy-ca.crt`.

### Mobile Devices

For mobile testing, run oproxy on a machine reachable from the device, then set the device Wi-Fi HTTP proxy to that machine's LAN IP and port `8080`.

This is intentionally not the default. To allow LAN clients:

```bash
OPROXY_BIND_HOST=0.0.0.0 cargo run --release
```

Use this only on a trusted network. The admin UI and management API are exposed wherever the proxy listener is exposed.

## Using The UI

The UI is available at `http://127.0.0.1:8080`.

### Sessions

The Sessions view is the main traffic log. Use it to:

- Filter by method, status, host, and text search.
- Switch between sequence and host/path structure views.
- Inspect overview, headers, request body, response body, timing, decoded payloads, and cookies.
- Copy a redacted cURL command.
- Explicitly copy raw cURL when you need unredacted local data.
- Replay a request.
- Open a captured request in Compose.
- Export selected sessions as HAR.

Sensitive values are masked in display and default export/copy flows.

### Compose

Compose is for manual request work:

- Create request tabs.
- Set method, URL, headers, query params, raw body, and content type.
- Use variables like `{{base_url}}`.
- Send through `/admin/forward`.
- Copy generated cURL.
- Save requests into local collections.

Compose collections and variables are saved in browser local storage for the
current admin UI origin. They survive page reloads and browser restarts on the
same machine/profile, but they are not written into the server `storage_path`.

### Rules And Traffic Controls

Use these surfaces when you need to change how traffic behaves:

- Routes: map a host to a different upstream destination.
- Rewrites: add/remove/replace headers, replace bodies, redirect, or block matching traffic.
- Header maps: set, append, or remove request headers by host/path/all scopes.
- Modifications: replace request headers or body for matching request URIs.
- Map local: return a local file instead of forwarding a host.
- Throttle: add latency and simulate bandwidth limits.
- Breakpoints: pause matching requests/responses and resume, modify, or drop.
- Mock server: return configured responses for matching method/host/path patterns.
- Lua scripts: modify or abort requests with sandboxed Lua.
- DNS override: resolve hostnames to fixed IPs before forwarding.
- Capture filter: record all, allowlist, or denylist hosts while still proxying traffic.
- Webhooks: POST events to local tooling when sessions complete.

When running in Docker, map-local file paths are paths inside the container. Mount a host directory into the container if you want to serve files from your workstation.

## HTTPS MITM And CA Handling

oproxy generates a local root CA under `certs` or the Docker `oproxy-certs` volume.

Download it:

```bash
curl http://127.0.0.1:8080/admin/ca -o oproxy-ca.crt
```

Trust it only on machines and devices you control. Anyone with the CA private key can impersonate TLS sites trusted by that CA. Protect the `certs` directory or Docker volume accordingly.

If you delete `certs`, oproxy will generate a new CA on the next start. You then need to remove the old CA from your system/browser trust store and trust the new one.

## Configuration

Configuration precedence:

1. Environment variables
2. `OPROXY_CONFIG` YAML file, or `./configs/default.yaml` if present
3. Built-in defaults

Common settings:

| Setting | Environment variable | Built-in default | Checked-in config |
| --- | --- | ---: | ---: |
| HTTP/UI port | `OPROXY_PORT` | `8080` | `8080` |
| Bind host | `OPROXY_BIND_HOST` | `127.0.0.1` | `127.0.0.1` |
| HTTPS MITM | `OPROXY_MITM_ENABLED` | `false` | `true` |
| Storage path | `OPROXY_STORAGE_PATH` | `./storage` | `./storage` |
| SOCKS5 port | config only | disabled | `1080` |
| Max body bytes per message | `OPROXY_MAX_BODY_BYTES` or hot reload | `10485760` | `10485760` |
| Max sessions | `OPROXY_MAX_SESSIONS` | `10000` | `10000` |
| Retained body budget | `OPROXY_MAX_RETAINED_BODY_BYTES` | `67108864` | `67108864` |

The checked-in `configs/default.yaml` is developer-friendly and enables MITM and SOCKS5. If you want the most conservative local posture, set `OPROXY_MITM_ENABLED=false` and remove or comment `socks5_port`.

### LAN Exposure

The safe default is loopback only. To expose oproxy to other machines, set:

```bash
OPROXY_BIND_HOST=0.0.0.0
```

In Docker, keep the host port binding loopback-only unless you really want LAN access:

```yaml
ports:
  - "127.0.0.1:8080:8080"
```

Publishing `0.0.0.0:8080:8080` exposes the proxy and admin API to your network.

## Management API

The management API is served by the same listener as the UI.

| Endpoint | Method | Purpose |
| --- | --- | --- |
| `/api/sessions` | `GET` | List captured sessions |
| `/api/sessions/:id` | `GET` | Read one captured session |
| `/api/sessions/:id/export?format=curl` | `GET` | Export one request as redacted cURL |
| `/api/sessions/:id/export?format=curl&raw=true` | `GET` | Export one request as raw cURL |
| `/api/import/curl` | `POST` | Parse a cURL command into request fields |
| `/admin/sessions` | `GET/DELETE` | List or clear captured sessions |
| `/admin/sessions/import` | `POST` | Import oproxy JSON sessions |
| `/admin/sessions/save` | `POST` | Save current sessions to a server-side file path |
| `/admin/sessions/load` | `POST` | Load sessions from a server-side file path |
| `/admin/sessions/export/har` | `GET` | Export redacted HAR |
| `/admin/sessions/export/har?raw=true` | `GET` | Export raw HAR |
| `/admin/sessions/import/har?merge=false` | `POST` | Import HAR, optionally replacing the current session log |
| `/admin/forward` | `POST` | Send a composed/replayed request |
| `/admin/routes` | `GET/POST` | Routing rules |
| `/admin/rewrites` | `GET/POST` | Rewrite rules |
| `/admin/header-maps` | `GET/POST` | Header map rules |
| `/admin/modifications` | `GET/POST` | Request modification rules |
| `/admin/map-local` | `GET/POST` | Serve local files for mapped hosts |
| `/admin/throttling` | `GET/POST` | Latency/bandwidth simulation |
| `/admin/breakpoints` | `GET/POST` | Breakpoint rules |
| `/admin/mock/rules` | `GET/POST` | Mock response rules |
| `/admin/scripts` | `GET/POST` | Lua scripts |
| `/admin/webhooks` | `GET/POST` | Webhook sinks |
| `/admin/dns` | `GET/POST` | DNS overrides |
| `/admin/capture-filter` | `GET/POST` | Capture allowlist/denylist filters |
| `/admin/config` | `GET` | Runtime configuration |
| `/admin/ca` | `GET` | Download local CA certificate |
| `/admin/metrics` | `GET` | Local capture counts, source breakdown, latency samples, endpoint timings, and byte totals |
| `/admin/socks5/status` | `GET` | SOCKS5 listener status |

Example replay/composed request:

```bash
curl -X POST http://127.0.0.1:8080/admin/forward \
  -H 'content-type: application/json' \
  -d '{
    "method": "GET",
    "url": "https://example.com",
    "headers": {},
    "body": null
  }'
```

Metrics count fields are intentionally split and also grouped under `sessions` and `requests`
so the values can be reconciled with `/api/sessions`:

- `captured_session_count`: all sessions currently in the workbench.
- `active_requests`: sessions without a recorded response yet.
- `proxied_requests`: sessions captured from the proxy listener.
- `admin_forward_requests`: requests sent through Compose/replay via `/admin/forward`.
- `inspected_requests`: completed sessions with timing metrics.
- `sessions.by_source`: source breakdown for `proxy`, `admin_forward`, `playback`, and `imported`.
- `endpoint_timings`: rolling local timings for management endpoints such as `/api/sessions` and `/admin/metrics`, including recent samples and per-endpoint last/average/max durations.

The old ambiguous aliases `total_requests` and `active_sessions` are not emitted; use
`captured_session_count` and `active_requests` instead.

## Persistence Model

oproxy keeps live captured sessions in memory by default. They survive while the
process runs and are cleared by `DELETE /admin/sessions` or process exit. To keep
session data across restarts, explicitly export HAR or call `/admin/sessions/save`
with a file path under a persistent location such as `/app/storage/sessions.json`,
then restore it with `/admin/sessions/load`.

The server `storage_path` persists runtime configuration and rule state:

- CA files are persisted under `mitm.root_ca_path`.
- Routes, rewrites, header maps, response modifications, map-local rules,
  throttling, breakpoints, DNS overrides, capture filter config, upstream proxy,
  mock rules, Lua scripts, and webhooks are loaded from and saved to `storage_path`.
- Hot config such as max retained body bytes is also stored under `storage_path`.
- Docker users should keep the named `/app/storage` and `/app/certs` volumes if
  they want these files to survive container replacement.

Compose collections and variables are client-side UI state. They are persisted in
browser local storage for `http://127.0.0.1:8080` or whichever admin origin you
use, not in the Docker volume.

## Privacy And Secret Handling

oproxy does not send telemetry.

Captured traffic, generated CA files, rules, scripts, webhooks, and Compose
collections are local to your machine. Treat the server storage path, Docker
volumes, browser profile, and exported session/HAR files as sensitive because
they can contain headers, cookies, tokens, request bodies, and response bodies.

By default, oproxy masks common sensitive keys in UI display and default copy/export flows:

- `authorization`
- `cookie`
- `set-cookie`
- `x-api-key`
- `api_key`
- `access_token`
- `refresh_token`
- `password`
- `secret`
- `token`

Raw local data is preserved for replay and explicit raw export/copy actions. Use raw export only when you understand where that data will go.

## Reset And Uninstall

Local source run:

```bash
rm -rf storage certs
```

Docker volumes:

```bash
docker volume rm oproxy_oproxy-storage oproxy_oproxy-certs
```

If you trusted the CA, also remove it from your operating system, browser, mobile device, or language runtime trust store.

## Troubleshooting

### UI is blank

Build the UI:

```bash
npm install --prefix src/design
npm run build --prefix src/design
```

Then restart oproxy. The server expects `src/design/dist/index.html`, `assets/app.js`, and `assets/app.css`.

### Browser traffic does not appear

Check that the browser proxy is set for both HTTP and HTTPS. Some browsers bypass proxies for localhost by default. Try a non-local host first, such as `http://example.com`.

### HTTPS fails

Confirm:

- MITM is enabled.
- The client trusts the CA from `/admin/ca`.
- The CA persisted across restarts. If not, download and trust the new CA.
- The client is not pinning certificates. Certificate-pinned apps may reject MITM by design.

### Docker starts but the UI is unreachable

Inside the container, oproxy must bind to `0.0.0.0`. The Dockerfile and compose file set `OPROXY_BIND_HOST=0.0.0.0`. The host port mapping should still usually be loopback-only: `127.0.0.1:8080:8080`.

### SOCKS5 is not listening

Check:

```bash
curl http://127.0.0.1:8080/admin/socks5/status
```

Set `socks5_port: 1080` in config to enable it.

### Requests are not recorded

Check Capture Filter. In allowlist mode, only matching hosts are recorded. In denylist mode, matching hosts are proxied but skipped in the session log.

## Development

Build the UI:

```bash
npm install --prefix src/design
npm run build --prefix src/design
```

Run tests:

```bash
RUSTFLAGS="-D warnings" cargo test
```

Build a release Docker image:

```bash
docker build -t oproxy:smoke .
```

Recommended release checks:

- `RUSTFLAGS="-D warnings" cargo test`
- `npm run build --prefix src/design`
- Docker build
- Docker constrained smoke test for HTTP capture, metrics, bounded storage, and UI health
- Browser smoke test against both the dev binary and the bundled Docker image
- HTTPS MITM and SOCKS5 tunnel forwarding checks
- Check there are no external runtime asset requests
- Start from clean storage with no demo/mock seeded state
- Verify redaction for UI, cURL, and HAR exports

## Contributing

Issues and pull requests are welcome. Useful contributions include:

- Reproducible bug reports with the client command, proxy configuration, and relevant logs.
- Browser or proxy E2E tests for a broken workflow.
- UI improvements that keep the app usable as a dense developer tool.
- Documentation updates when behavior changes.

Before opening a pull request, run:

```bash
RUSTFLAGS="-D warnings" cargo test
npm run build --prefix src/design
```

Please do not include captured real secrets, private CA keys, or production traffic dumps in issues or fixtures.

## License

MIT
