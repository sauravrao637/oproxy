# Configuration

Configuration precedence is:

1. supported environment variables
2. YAML config file from `OPROXY_CONFIG`, or `./configs/default.yaml`
3. built-in defaults

## Supported Options

| YAML field | Environment variable | Built-in default | Checked-in config |
| --- | --- | --- | --- |
| `port` | `OPROXY_PORT` | `8080` | `8080` |
| `bind_host` | `OPROXY_BIND_HOST` | `127.0.0.1` | `127.0.0.1` |
| `mitm.enabled` | `OPROXY_MITM_ENABLED` | `false` | `true` |
| `mitm.root_ca_path` | none | `./certs` | `./certs` |
| `storage_path` | `OPROXY_STORAGE_PATH` | `./storage` | `./storage` |
| `log.level` | `OPROXY_LOG_LEVEL`, `RUST_LOG` | `info` | `info` |
| `log.dir` | `OPROXY_LOG_DIR` | `.` | `.` |
| `log.file` | none | `server.log` | `server.log` |
| `timeout_secs` | none | `30` | `30` |
| `connect_timeout_secs` | `OPROXY_CONNECT_TIMEOUT_SECS` | `10` | `10` |
| `handshake_timeout_secs` | `OPROXY_HANDSHAKE_TIMEOUT_SECS` | `10` | `10` |
| `shutdown_grace_secs` | `OPROXY_SHUTDOWN_GRACE_SECS` | `10` | `10` |
| `max_body_bytes` | `OPROXY_MAX_BODY_BYTES` | `10485760` | `10485760` |
| `stream_threshold_bytes` | `OPROXY_STREAM_THRESHOLD_BYTES` | `524288` | `524288` |
| `pool_max_idle_per_host` | none | `10` | `10` |
| `pool_idle_timeout_secs` | none | `30` | `30` |
| `max_sessions` | `OPROXY_MAX_SESSIONS` | `10000` | `10000` |
| `max_retained_body_bytes` | `OPROXY_MAX_RETAINED_BODY_BYTES` | `67108864` | `67108864` |
| `max_connections` | `OPROXY_MAX_CONNECTIONS` | `1024` | `1024` |
| `https_port` | `OPROXY_HTTPS_PORT` | unset | unset |
| `inspect_ws_frames` | `OPROXY_INSPECT_WS_FRAMES` | `true` | `true` by default |
| `allow_remote_admin` | `OPROXY_ALLOW_REMOTE_ADMIN` | `false` | `false` |
| `admin_token` | `OPROXY_ADMIN_TOKEN` | unset | unset |
| `allow_private_admin_egress` | `OPROXY_ALLOW_PRIVATE_ADMIN_EGRESS` | `false` | `false` |
| `upstream_proxy` | none | unset | unset |
| `socks5_port` | `OPROXY_SOCKS5_PORT` | unset | `1080` |
| `map_local_base_path` | `OPROXY_MAP_LOCAL_BASE_PATH` | unset | unset |
| `advertised_host` | `OPROXY_ADVERTISED_HOST` | unset (auto-detect) | unset |
| `http3_enabled` | `OPROXY_HTTP3_ENABLED` | `false` | unset (commented out) |
| `http3_port` | `OPROXY_HTTP3_PORT` | unset | unset (commented out) |
| `otel_enabled` | `OPROXY_OTEL_ENABLED` | `false` | unset |
| `otel_endpoint` | `OPROXY_OTEL_ENDPOINT` | unset | unset |
| `update_check` | `OPROXY_UPDATE_CHECK` | `true` | unset |
| none (env-only) | `OPROXY_INSECURE_UPSTREAM` | `0` (verification on) | unset |

`OPROXY_CONFIG` selects the YAML file itself.

`OPROXY_INSECURE_UPSTREAM` has no YAML field or `Config` struct entry - it's
read directly from the process environment (`1`/`true`/`yes` to disable
upstream TLS certificate verification), independent of the config-file/env
precedence used by everything else in this table. See "TLS verification" in
the Docker Compose file for the tradeoffs.

`otel_enabled`/`otel_endpoint` only take effect in binaries built with the
`otel` Cargo feature; `http3_enabled`/`http3_port` require the `http3`
feature (the Docker image is built with `--all-features`, so both are
available there).

### `max_body_bytes` and large request/response bodies

`max_body_bytes` caps how much of a single request or response body is
*buffered in memory* for inspection/rewrite - it does not cap what's actually
forwarded:

- A **request** whose `Content-Length` is already known and exceeds
  `max_body_bytes` is streamed straight through to the upstream server
  unbuffered, instead of being rejected. A chunked request (no
  `Content-Length`) that only turns out to be large mid-upload is still
  buffered up to the cap and rejected with `413` if it's exceeded - only the
  known-length case is streamed today.
- A **response** that is `text/event-stream`, chunked, or larger than
  `stream_threshold_bytes` (default 512 KB, `OPROXY_STREAM_THRESHOLD_BYTES`)
  is always streamed straight through to the client unbuffered, regardless of
  `max_body_bytes` (the streamed body is separately teed into a capped copy
  for the recorded session, bounded by `max_body_bytes`). Chunked/SSE
  responses stream unconditionally regardless of this threshold; it only
  controls the size cutoff for responses that declare a `Content-Length`.

In both streaming cases, body-mutating middleware (rewrite `replace_body`,
Mock, Lua) cannot act on a body it never sees in full, so it has no effect.
The recorded session is tagged `streamed` so this is visible rather than a
silent no-op. Raising `stream_threshold_bytes` lets more responses be
buffered (and therefore inspected/rewritten) before falling back to
streaming, at the cost of buffering more per exchange - set it in step with
`max_body_bytes` if you need larger responses to be capturable.

### `advertised_host` and the setup wizard in containers

The setup wizard (`/setup`, its QR code, `GET /admin/setup/network-info`)
needs a LAN address that a phone or another machine can actually reach. By
default it auto-detects one by asking the OS which local interface it would
use to reach the internet. Inside a container that reports the container's own
bridge/internal address (e.g. `172.17.0.2`), which nothing outside the
container can reach - there's no way to detect the real host-reachable address
from inside the container's network namespace. Set `advertised_host`
(`OPROXY_ADVERTISED_HOST`) to the address phones/browsers should actually use
(typically the Docker host's LAN IP) to fix this; native and host-networked
deployments don't need it, since auto-detection already returns a reachable
address there.

## Example Minimal Config

```yaml
port: 8080
bind_host: "127.0.0.1"
mitm:
  enabled: true
  root_ca_path: ./certs
storage_path: ./storage
```

## Example Full Config

```yaml
port: 8080
bind_host: "127.0.0.1"
allow_remote_admin: false
admin_token:
allow_private_admin_egress: false

mitm:
  enabled: true
  root_ca_path: ./certs

storage_path: ./storage

log:
  level: info
  dir: .
  file: server.log

timeout_secs: 30
connect_timeout_secs: 10
handshake_timeout_secs: 10
shutdown_grace_secs: 10
max_body_bytes: 10485760
stream_threshold_bytes: 524288
pool_max_idle_per_host: 10
pool_idle_timeout_secs: 30
max_connections: 1024
https_port:
inspect_ws_frames: true
socks5_port: 1080
max_sessions: 10000
max_retained_body_bytes: 67108864
upstream_proxy:
map_local_base_path: /map-local
```


## Environment Variables

Invalid environment values are fatal by design: a malformed value (e.g.
`OPROXY_PORT=notaport` or `OPROXY_MITM_ENABLED=maybe`) causes the process to
panic and exit immediately at startup, rather than silently falling back to a
default. This is intentional - a typo'd env var should be loud and immediate
(a crash-looping container with a clear panic message) rather than a proxy
that silently runs with different settings than the operator intended. In
Docker/Compose this surfaces as the container failing to start; check the
container logs for a line like `Environment variable OPROXY_PORT has invalid
value 'notaport': invalid digit found in string`.

