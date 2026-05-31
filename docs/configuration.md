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
| `socks5_port` | none | unset | `1080` |

`OPROXY_CONFIG` selects the YAML file itself.

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
pool_max_idle_per_host: 10
pool_idle_timeout_secs: 30
max_connections: 1024
https_port:
inspect_ws_frames: true
socks5_port: 1080
max_sessions: 10000
max_retained_body_bytes: 67108864
upstream_proxy:
```


## Environment Variables

Invalid environment values are ignored with a warning.

