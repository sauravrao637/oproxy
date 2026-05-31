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
  -v oproxy-certs:/app/certs \
  -v oproxy-storage:/app/storage \
  oproxy:latest
```

Docker port publishing needs the process inside the container to bind to `0.0.0.0`. The host mappings above expose the service only on host loopback.

## Docker Compose

```bash
docker compose up --build
```

The checked-in `docker-compose.yml` uses:

- `network_mode: host`
- `OPROXY_BIND_HOST=0.0.0.0`
- `OPROXY_MITM_ENABLED=true`
- `OPROXY_ALLOW_REMOTE_ADMIN=false`
- `oproxy-certs:/app/certs`
- `oproxy-storage:/app/storage`

Because it uses host networking, the Compose file does not declare a `ports:` block.

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

Live captured sessions are kept in memory unless you export HAR or explicitly save sessions with `/admin/sessions/save`.

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
  -v oproxy-certs:/app/certs \
  -v oproxy-storage:/app/storage \
  oproxy:latest
```

Keep the CA volume if clients already trust the current CA. Replacing it creates a new CA and requires reinstalling trust.

