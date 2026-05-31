# HTTPS MITM

oproxy can inspect HTTPS traffic by acting as a local certificate authority and generating per-domain certificates during CONNECT handling.

## How CA Generation Works

On startup, oproxy initializes a root CA at `mitm.root_ca_path`.

Default built-in path:

```yaml
mitm:
  enabled: false
  root_ca_path: ./certs
```

The checked-in `configs/default.yaml` sets `mitm.enabled: true`.

CA files:

- `root.crt`
- `root.key`

If both files exist, oproxy loads the existing key and reconstructs the CA certificate. If either is missing, it generates a new CA. On Unix systems, `root.key` is written with owner-only permissions.

Domain certificates are generated on demand and cached in memory, up to 1024 entries.

## Certificate Installation

Download the CA:

```bash
curl http://127.0.0.1:8080/admin/ca -o oproxy-ca.crt
```

Use it with curl:

```bash
curl --cacert oproxy-ca.crt -x http://127.0.0.1:8080 https://example.com
```


## Limitations

- HTTPS inspection requires `mitm.enabled: true` or `OPROXY_MITM_ENABLED=true`.
- The client must trust the oproxy CA.
- Certificate-pinned applications may reject the generated certificate.
- Deleting `root.key` or `root.crt` changes the CA and invalidates previously installed trust.
- The CA endpoint returns `404` only if CA initialization is unavailable; runtime initialization normally creates the CA even when MITM is disabled.
- SOCKS5 captures sessions only when MITM is active and the target port is treated as TLS.

## Security Considerations

The CA private key can sign certificates trusted by any client that installed `root.crt`. Protect `root.key`, Docker CA volumes, backups, and exported files.

Remove the oproxy CA from trust stores when you no longer need it. Do not install the CA on devices you do not control.

