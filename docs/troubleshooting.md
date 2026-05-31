# Troubleshooting

## UI Is Blank

Build the UI assets:

```bash
corepack enable
yarn --cwd src/design install --frozen-lockfile
yarn --cwd src/design build
cargo run --release
```

Build the UI assets explicitly before running from source.

## Browser Traffic Does Not Appear

Check the browser proxy settings:

- HTTP proxy: `127.0.0.1`, port `8080`
- HTTPS proxy: `127.0.0.1`, port `8080`

Try a non-local target:

```bash
curl -x http://127.0.0.1:8080 http://example.com
curl 'http://127.0.0.1:8080/api/sessions?limit=5'
```

Check capture filter:

```bash
curl http://127.0.0.1:8080/admin/capture-filter
```

Allowlist mode records only matching hosts. Denylist mode proxies matching hosts but skips recording.

## HTTPS Fails

Check:

```bash
curl http://127.0.0.1:8080/admin/config
curl http://127.0.0.1:8080/admin/ca -o oproxy-ca.crt
curl --cacert oproxy-ca.crt -x http://127.0.0.1:8080 https://example.com
```

Common causes:

- MITM is disabled.
- The client does not trust the CA from `/admin/ca`.
- The CA volume or `certs` directory changed, so the client trusts an old CA.
- The target app uses certificate pinning.

## Docker UI Is Unreachable

When using `docker run` with port publishing, the process inside the container must bind to `0.0.0.0`:

```bash
docker run --rm \
  -p 127.0.0.1:8080:8080 \
  -e OPROXY_BIND_HOST=0.0.0.0 \
  oproxy:latest
```

The checked-in Compose file uses host networking and sets `OPROXY_BIND_HOST=0.0.0.0`.

## SOCKS5 Is Not Listening

Check:

```bash
curl http://127.0.0.1:8080/admin/socks5/status
```

SOCKS5 is enabled only when `socks5_port` is set in YAML. The built-in default is disabled; `configs/default.yaml` sets `socks5_port: 1080`.

## Admin API Returns 403 On Forwarding Or Webhooks

When remote admin is enabled, admin-initiated egress to private, loopback, link-local, multicast, and unspecified IP ranges is blocked unless `allow_private_admin_egress` is true.

Check:

```bash
curl http://127.0.0.1:8080/admin/config
```

## Diagnostic Commands

```bash
curl http://127.0.0.1:8080/health
curl http://127.0.0.1:8080/admin/config
curl http://127.0.0.1:8080/admin/metrics
curl http://127.0.0.1:8080/admin/plugins
curl http://127.0.0.1:8080/admin/socks5/status
```

Run tests:

```bash
RUSTFLAGS="-D warnings" cargo test
yarn --cwd src/design build
```
