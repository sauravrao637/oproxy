# Security

oproxy is a local developer proxy. It captures, stores, rewrites, and replays HTTP traffic, so treat its state as sensitive.

## Threat Model

Trust boundary:

- local users who can access the UI/API listener
- clients configured to use oproxy as a proxy
- anyone with access to `storage_path`, `mitm.root_ca_path`, Docker volumes, browser local storage, logs, or exported capture files

Primary risks:

- captured secrets in headers and bodies
- CA private key exposure
- remote administration of proxy controls
- admin-triggered requests to internal networks
- Lua scripts modifying traffic
- webhooks sending captured metadata to another service

## CA Handling

CA files live under `mitm.root_ca_path`, default `./certs`.

The private key is `root.key`. On Unix, oproxy writes it with mode `0600`.

Anyone with this key can create certificates trusted by clients that installed the oproxy CA. Protect:

- `certs/`
- Docker `oproxy-certs` volume
- backups
- copied CA material

If CA material changes, remove the old CA from trust stores and install the new one only where needed.

## Remote Administration Risks

Default built-in bind host is `127.0.0.1`, and `allow_remote_admin` defaults to `false`.

Binding to `0.0.0.0` exposes the proxy listener to the network. With `allow_remote_admin=false`, ordinary LAN Host headers are treated as proxy traffic rather than management traffic.

If you intentionally enable remote admin:

```yaml
bind_host: "0.0.0.0"
allow_remote_admin: true
admin_token: "change-me"
allow_private_admin_egress: false
```

Set a token. The server warns if remote admin is enabled without one.

When remote admin is enabled, `/admin/forward`, playback, and webhooks cannot target private, loopback, link-local, multicast, or unspecified IP ranges unless `allow_private_admin_egress` is true.

## Storage Considerations

Server-side `storage_path` persists rule and control-plane state. Live sessions are in memory unless explicitly saved or exported.

Sensitive locations:

- `storage_path`
- `mitm.root_ca_path`
- Docker volumes
- browser local storage for Compose collections and variables
- HAR exports
- raw cURL/Fetch/Python exports
- saved session JSON files
- webhook destination logs

Default exports redact common sensitive headers and body fields. Raw exports are intentionally unredacted.

## Lua Scripts

Lua scripts are stored in `lua_scripts.json` and run for each request/response while enabled.

The Lua environment removes selected globals such as `io`, `os`, `package`, `require`, `load`, `loadfile`, `dofile`, `debug`, and `coroutine`. Scripts still can modify traffic and abort requests, so only enable scripts you trust.

## Webhooks

Webhooks can be configured for `request_captured` and `response_captured`. Payloads include event type, session id, timestamp, request method, URI, and response status if present.

If a webhook secret is set, oproxy sends an `x-oproxy-signature` HMAC-SHA256 header over the JSON payload.
