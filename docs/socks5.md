# SOCKS5

oproxy can start an optional SOCKS5 listener with the `socks5_port` configuration field.

The checked-in `configs/default.yaml` enables:

```yaml
socks5_port: 1080
```

Built-in defaults leave SOCKS5 disabled.

## Capabilities

Supported:

- SOCKS5 no-auth method
- CONNECT command
- IPv4 targets
- IPv6 targets
- domain-name targets
- DNS overrides before dialing
- plain TCP tunneling
- TLS MITM for TLS ports when MITM is enabled and a CA is available

Status endpoint:

```bash
curl http://127.0.0.1:8080/admin/socks5/status
```

Example response:

```json
{
  "enabled": true,
  "port": 1080,
  "mode": "mitm",
  "captures_sessions": true
}
```

## Example Clients

curl:

```bash
curl --socks5-hostname 127.0.0.1:1080 http://example.com
```

curl over HTTPS with MITM enabled and the CA trusted:

```bash
curl http://127.0.0.1:8080/admin/ca -o oproxy-ca.crt
curl --cacert oproxy-ca.crt --socks5-hostname 127.0.0.1:1080 https://example.com
```

Environment variable for clients that support it:

```bash
export ALL_PROXY=socks5h://127.0.0.1:1080
```

## Limitations

- Only no-auth SOCKS5 is implemented.
- Only CONNECT is implemented; BIND and UDP ASSOCIATE are rejected.
- In tunnel-only mode, SOCKS5 does not capture full HTTP sessions.
- `captures_sessions` is true only when SOCKS5 is enabled and MITM is active.
- SOCKS5 has no environment variable override; configure `socks5_port` in YAML.

