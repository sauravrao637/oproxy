# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project aims to
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/sauravrao637/oproxy/compare/v0.1.9...HEAD
[0.1.9]: https://github.com/sauravrao637/oproxy/compare/v0.1.8...v0.1.9
