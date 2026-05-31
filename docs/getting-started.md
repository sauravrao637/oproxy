# Getting Started

oproxy runs a local proxy and management UI on the same listener. The default UI URL is `http://127.0.0.1:8080`.

## Installation

### Docker

```bash
docker build -t oproxy:latest .
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

Open `http://127.0.0.1:8080`.

### Source

```bash
corepack enable
yarn --cwd src/design install --frozen-lockfile
yarn --cwd src/design build
cargo run --release
```

If `configs/default.yaml` is present, the source run uses it by default. That checked-in file enables MITM and SOCKS5.

## First Request

Run this in another terminal:

```bash
curl -x http://127.0.0.1:8080 http://example.com
```

Open the Sessions view in the UI. You should see a `GET http://example.com/` capture.

## First HTTPS Capture

Download the generated CA certificate:

```bash
curl http://127.0.0.1:8080/admin/ca -o oproxy-ca.crt
```

Use it with curl:

```bash
curl --cacert oproxy-ca.crt -x http://127.0.0.1:8080 https://example.com
```

For browsers, configure a manual HTTP and HTTPS proxy at `127.0.0.1:8080`, then import the CA from `/admin/ca` into the browser or OS trust store.

