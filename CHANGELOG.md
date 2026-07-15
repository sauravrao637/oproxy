# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project aims to
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.10] - 2026-07-15

### Added
- Tags for mocked/replayed/rewritten sessions and actionable upstream-error response bodies.
- Configurable large-response streaming threshold (`OPROXY_STREAM_THRESHOLD_BYTES`) and a
  consolidated startup security posture banner.
- `OPROXY_ADVERTISED_HOST` override for the setup wizard behind reverse proxies/containers.
- Soak/load test harness (`make soak-test`) and a `cargo xtask` crate for repo automation
  (setup, UI build, release-version and dist checks).

### Fixed
- Streamed responses are now recorded, and oversized uploads are streamed instead of
  rejected with 413.
- MITM'd WebSocket upgrades route through the upgrade-aware handler; MITM cert validity
  windows are capped, and the persisted root CA is reused instead of regenerated on restart.
- Map Local, DNS override, and several other API/config validation papercuts.
- Docker Compose defaults to bridge networking with a healthcheck; loopback Host headers are
  trusted correctly under opt-in remote-admin auth.
- UI: streamed exchanges show a "body not captured" indicator, tiny-viewport detection is
  debounced, and session tag naming matches the UI ("replay"/"rewrite").

## [0.1.9] - 2026-06-15

### Added
- `examples/custom_middleware.rs` and `examples/embed_proxy.rs` showing how to
  write a middleware plugin and embed the proxy engine.
- Doctests on the marquee public APIs (`Middleware`, `HeaderMap`,
  `ProxyEngineConfig`).
- `[lints]` table in `Cargo.toml` so the warning policy is reproducible locally,
  not only in CI.
- Unified `ApiError` type for control-plane handlers.
- Field-scoped session search supporting `tag:`, `host:`, `method:`, and
  `status:` filters.

### Changed
- `ProxyEngine::new` now takes a `ProxyEngineConfig` struct instead of a long
  positional argument list.
- Split `core/engine.rs` into `core/engine/{mod,wire}.rs` and extracted the
  session search grammar into `session/search.rs`.
- Refactored config env-var tests onto an RAII `EnvGuard`, centralising `unsafe`.
- Configuration loading now fails fast when the config file is missing,
  malformed, or contains invalid environment overrides.
- Refactored proxy forwarding, transport lifecycles, runtime construction,
  assistant actions, HAR conversion, Lua execution, and session storage into
  smaller typed components.
- Standardized comments and removed stale implementation-phase wording.

### Removed
- Dead `MiddlewareAction::Pause` variant, the redundant `forward_class`/
  `select_class` helpers, and an unused WebSocket-over-h2 stub.

[Unreleased]: https://github.com/sauravrao637/oproxy/compare/v0.1.10...HEAD
[0.1.10]: https://github.com/sauravrao637/oproxy/compare/v0.1.9...v0.1.10
[0.1.9]: https://github.com/sauravrao637/oproxy/compare/v0.1.8...v0.1.9
