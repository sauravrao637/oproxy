# DNS Overrides

DNS overrides resolve configured hostnames to fixed IP strings before forwarding.

## How It Applies

For HTTP proxy traffic, oproxy:

1. strips the port from the request host for lookup;
2. replaces the host with the configured IP;
3. sets the destination to `https://IP:port` only when the original port is `443`, otherwise `http://IP:port`.

For CONNECT and SOCKS5, overrides are also checked before dialing the target.

## Caveats

- Matching is exact on the hostname key, not wildcard or substring.
- The override value is stored as a string and is not validated by the admin handler.
- In HTTP proxy traffic, scheme selection is based on port `443` versus all other ports.
- Overrides can affect Map Remote because DNS override runs before Map Remote in the request chain.
- Overrides are persisted in `dns_overrides.json` under `storage_path`.

