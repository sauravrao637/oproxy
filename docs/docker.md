# Docker

The Docker image builds the React UI, compiles the Rust binary, and runs as the `oproxy` system user from `/app`.

## Build

```bash
docker build -t oproxy:latest .
```

## docker run

Using the release image:

```bash
docker run --rm \
  --name oproxy \
  -p 127.0.0.1:8080:8080 \
  -p 127.0.0.1:1080:1080 \
  -e OPROXY_BIND_HOST=0.0.0.0 \
  -e OPROXY_MITM_ENABLED=true \
  -e OPROXY_ALLOW_REMOTE_ADMIN=true \
  -e OPROXY_ADMIN_TOKEN=change-me-to-a-strong-secret \
  -v oproxy-certs:/app/certs \
  -v oproxy-storage:/app/storage \
  ghcr.io/sauravrao637/oproxy:latest
```

Using a local build:

```bash
docker run --rm \
  --name oproxy \
  -p 127.0.0.1:8080:8080 \
  -p 127.0.0.1:1080:1080 \
  -e OPROXY_BIND_HOST=0.0.0.0 \
  -e OPROXY_MITM_ENABLED=true \
  -e OPROXY_ALLOW_REMOTE_ADMIN=true \
  -e OPROXY_ADMIN_TOKEN=change-me-to-a-strong-secret \
  -v oproxy-certs:/app/certs \
  -v oproxy-storage:/app/storage \
  oproxy:latest
```

Docker port publishing needs the process inside the container to bind to `0.0.0.0`. The host mappings above expose the service only on host loopback.

### Why `OPROXY_ALLOW_REMOTE_ADMIN` + `OPROXY_ADMIN_TOKEN` are needed here

Bridge networking (what `-p` uses) routes published-port traffic through Docker's
NAT before it reaches the container. That means the proxy never sees a
genuinely loopback peer address for this traffic — not even for a browser
running on the very same machine you ran `docker run` on — so the loopback-only
check that lets the Web UI through on a bare-metal install can never pass here.
Setting both of the flags above tells oproxy to trust `Host: 127.0.0.1`
requests through the published port, gated by the token instead of by peer
address. Without them, `http://127.0.0.1:8080/` and the rest of the admin
UI/API 502 with "Proxy loop detected", even though proxying itself (`curl -x
http://127.0.0.1:8080 http://example.com`) works fine. Open
`http://127.0.0.1:8080/` and sign in with the token when prompted, or append
`?token=change-me-to-a-strong-secret` to any admin URL to authenticate that
request directly (see `docs/security.md` for the full list of accepted token
locations). If you only ever access oproxy from inside the container itself,
or you switch to `network_mode: host` (Linux only), you can leave
`OPROXY_ALLOW_REMOTE_ADMIN` at its default `false` — the loopback peer check
is accurate again in both of those cases.

## Docker Compose

```bash
docker compose up --build
```

The checked-in `docker-compose.yml` uses:

- Bridge networking with a `ports:` block, published on host loopback
  (`127.0.0.1:8080`, `127.0.0.1:8443` TCP+UDP)
- `OPROXY_BIND_HOST=0.0.0.0`
- `OPROXY_MITM_ENABLED=true`
- `OPROXY_HTTP3_ENABLED=true` + `OPROXY_HTTP3_PORT=8443`
- `OPROXY_ALLOW_REMOTE_ADMIN=true` + `OPROXY_ADMIN_TOKEN` (placeholder - change it)
- `oproxy-certs:/app/certs`
- `oproxy-storage:/app/storage`
- A `healthcheck:` hitting `/health`, so a crash-looping container (e.g. a
  typo'd env var - invalid values are fatal by design, see
  `docs/configuration.md`) shows as `unhealthy` in `docker compose ps` instead
  of a silent restart loop

Bridge networking (the default here) works on every Docker platform, including
Docker Desktop for Windows/macOS. `network_mode: host` is commented out as an
alternative — it lets the proxy see real client IPs with no port mapping, but
only works on Linux hosts. On Docker Desktop, host networking maps into the
Docker Desktop VM rather than the actual Windows/macOS host, so the container's
ports are never reachable from the host at all in that mode. If you
switch to `network_mode: host` on Linux, you can also set
`OPROXY_ALLOW_REMOTE_ADMIN` back to `false`, since the loopback peer check is
accurate again without Docker's port-publishing NAT in the way. See "Why
`OPROXY_ALLOW_REMOTE_ADMIN` + `OPROXY_ADMIN_TOKEN` are needed here" above for
why bridge mode requires them for the Web UI.

If you access oproxy from a phone or another LAN machine, also set
`OPROXY_ADVERTISED_HOST` to your host machine's real LAN IP - auto-detection
from inside the container otherwise reports the container's own bridge address
to the setup wizard/QR code, which isn't reachable from outside the container.

### HTTP/3 (QUIC)

The Docker image is built with `--all-features`, so the `http3` listener is
available out of the box. The Compose file enables it on UDP port 8443; with
host networking it is reachable directly. For bridge networking (or
`docker run`), publish the UDP port explicitly, e.g. `-p 127.0.0.1:8443:8443/udp`.
Clients must trust the oproxy root CA (`/admin/ca`) for QUIC TLS, and forwarded
responses advertise the listener via `alt-svc: h3=":8443"`.

## Protocol Fixtures

For local protocol testing without external services, start the fixture profile:

```bash
docker compose --profile fixtures up --build
```

This starts `oproxy` plus stable local fixture origins:

| Fixture | URL / target |
| --- | --- |
| HTTP/1.1 | `http://127.0.0.1:18080/` |
| HTTP/2 TLS | `https://127.0.0.1:18443/` |
| WebSocket echo | `ws://127.0.0.1:18081/` |
| gRPC TLS echo | `127.0.0.1:19090`, service `echo.EchoService` |

The fixture container writes self-signed origin CAs to the
`protocol-fixture-certs` Docker volume. When clients connect through oproxy with
MITM enabled, install/trust the oproxy CA from `/admin/ca`. When clients connect
directly to the HTTP/2 or gRPC fixture origins, trust the matching fixture CA
from that volume instead.

The fixture ports are published on host loopback, matching the oproxy service's
own bridge-mode port publishing, so both containers, browser tests, and local
CLI clients can all target the same `127.0.0.1:*` addresses regardless of
which oproxy networking mode is active. If you switch oproxy to
`network_mode: host` (Linux only), reach the fixtures via the same
`127.0.0.1:*` addresses from the host, or via the `protocol-fixtures` service
name from inside another bridge-networked container.

## Volumes

`/app/certs` stores the generated root CA files:

- `root.crt`
- `root.key`

`/app/storage` stores persisted control-plane state:

- `rule_sets.json`
- `map_remote_rules.json`
- `map_local_rules.json`
- `access_rules.json`
- `throttle.json`
- `dns_overrides.json`
- `breakpoints.json`
- `capture_filter.json`
- `upstream_proxy.json`
- `hot_config.json`
- `lua_scripts.json`
- `mock_rules.json`
- `webhooks.json`

Live captured sessions are kept in memory unless you export HAR or explicitly save sessions with `/admin/sessions/save`. `POST /admin/sessions/save` and `/admin/sessions/load` both take an optional `{"path": "<name>"}` body; if omitted (or posted as `{}`), they default to `sessions.json` under `storage_path`.

## Upgrades

Build or pull the new image, then recreate the container with the same named volumes.

```bash
docker build -t oproxy:latest .
docker stop oproxy || true
docker run --rm \
  --name oproxy \
  -p 127.0.0.1:8080:8080 \
  -p 127.0.0.1:1080:1080 \
  -e OPROXY_BIND_HOST=0.0.0.0 \
  -e OPROXY_MITM_ENABLED=true \
  -e OPROXY_ALLOW_REMOTE_ADMIN=true \
  -e OPROXY_ADMIN_TOKEN=change-me-to-a-strong-secret \
  -v oproxy-certs:/app/certs \
  -v oproxy-storage:/app/storage \
  oproxy:latest
```

Keep the CA volume if clients already trust the current CA. Replacing it creates a new CA and requires reinstalling trust.
