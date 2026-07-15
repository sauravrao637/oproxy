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

## HTTPS Fails On Windows With `CERT_TRUST_REVOCATION_STATUS_UNKNOWN`

Windows/Schannel-based clients (`curl.exe`, .NET `HttpClient`, and some other
system components) validate certificate revocation status by default.
oproxy's generated MITM certificates don't carry a CRL Distribution Point or
OCSP URL, so Schannel treats revocation as "required but unavailable" and
rejects the connection with an error like:

```text
schannel: CertGetCertificateChain trust error CERT_TRUST_REVOCATION_STATUS_UNKNOWN
```

This doesn't affect OpenSSL-based clients (curl on Linux/macOS, most
browsers), only Schannel-based ones. Workarounds:

- `curl.exe --ssl-revoke-best-effort --cacert oproxy-ca.crt -x http://127.0.0.1:8080 https://example.com`
- In .NET, disable revocation checking on the `HttpClientHandler`/`SslOptions`
  used for the request (e.g. `CertificateRevocationCheckMode.NoCheck`).

Generated certificates already omit CRL Distribution Points and Authority Information Access extensions. `rcgen` provides no additional flag to declare the absence of revocation information.
Fabricating a CRL/AIA extension that points nowhere would require
hand-built ASN.1/DER (rcgen has no typed API for it) and there's no
evidence it changes Schannel's behavior - Schannel treats "cannot
determine revocation status" as a failure when
`CheckCertificateRevocationList`/equivalent is on, regardless of
whether that's because no extension is present or because an extension
points at an unreachable responder. Other MITM tools (mitmproxy,
Charles) rely on the same absence-based default and document the same
client-side workarounds above rather than adding cert extensions.

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

SOCKS5 is enabled only when `socks5_port` is set (via YAML or the `OPROXY_SOCKS5_PORT` env var, which overrides YAML). The built-in default is disabled; `configs/default.yaml` sets `socks5_port: 1080`.

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
