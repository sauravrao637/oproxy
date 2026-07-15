# Contributing to oproxy

Thank you for your interest in contributing to oproxy! This document provides guidelines and information for contributors.

## Code of Conduct

This project and everyone participating in it is governed by our community standards. By participating, you are expected to uphold these standards.

## How Can I Contribute?

### Reporting Bugs

Before creating bug reports, please check the [existing issues](https://github.com/sauravrao637/oproxy/issues) to see if the problem has already been reported.

When filing a bug report using the [bug report template](https://github.com/sauravrao637/oproxy/issues/new?template=bug_report.yml), include:

- **Clear description** of the bug
- **Steps to reproduce** the behavior
- **Expected behavior** vs actual behavior
- **Environment details**: OS, oproxy version (Docker tag or commit hash), browser/client being proxied
- **Relevant logs** or screenshots

### Suggesting Features

Feature requests are welcome. Use the [feature request template](https://github.com/sauravrao637/oproxy/issues/new?template=feature_request.yml) and include:

- **Problem statement**: What limitation or problem are you trying to solve?
- **Proposed solution**: How would you like it to work?
- **Alternatives considered**: What other approaches did you consider?
- **Additional context**: Use cases, examples, or mockups

### Questions and Discussions

For questions, support, or general discussion, use [GitHub Discussions](https://github.com/sauravrao637/oproxy/discussions) instead of opening an issue.

## Development Setup

### Prerequisites

- **Rust**: 1.85 or newer (edition 2024)
- **Node.js**: 22 or newer
- **Yarn**: 4.15.0 (managed via Corepack)

### Initial Setup

1. **Clone the repository:**
   ```bash
   git clone https://github.com/sauravrao637/oproxy.git
   cd oproxy
   ```

2. **Install git hooks** (recommended):
   ```bash
   make setup
   # equivalent to: cargo xtask setup
   ```
   This checks for required tooling (Node, Python, bash, yarn) and installs
   a pre-commit hook that runs `cargo fmt --all -- --check`. On Windows,
   `cargo xtask setup` works from a plain cmd.exe/PowerShell — no Git
   Bash/WSL required.

3. **Build the UI:**
   ```bash
   make ui
   # equivalent to: cargo xtask build-ui
   ```

4. **Build and run:**
   ```bash
   cargo run --release
   ```
   
   Or for development:
   ```bash
   cargo run
   ```

5. **Test the proxy:**
   ```bash
   curl -x http://127.0.0.1:8080 http://example.com
   ```

### Docker Development

Build locally:
```bash
docker build -t oproxy:latest .
```

Or use Docker Compose:
```bash
docker compose up --build
```

## Development Workflow

### Building

```bash
# Debug build
make build

# Release build
make build-release

# Build UI only
make ui

# Verify src/design/dist has no dev-only/CDN leakage (also runs in CI before release)
make check-dist
```

### Repo automation (`cargo xtask`)

Tasks that need real logic rather than a one-line shell command live in the
`xtask` crate (`xtask/src/main.rs`) instead of the Makefile, so local dev and
CI run the exact same Rust code:

```bash
cargo xtask setup                         # same as `make setup` — works without bash on Windows
cargo xtask build-ui                      # same as `make ui`
cargo xtask check-dist                    # same as `make check-dist`
cargo xtask verify-release-version <tag>  # what CI checks before publishing a release
```

`xtask` is a workspace member but not a default one — it only builds when
you run `cargo xtask ...` explicitly, so it has no effect on `cargo build`
or `cargo test`. The `make` targets above are thin wrappers around it; use
whichever you're more comfortable with. If you're adding new automation
that involves more than a couple of lines of shell (parsing files, walking
directories, anything you'd want a unit test for), prefer adding a
subcommand to `xtask` over growing the Makefile.

### Formatting

All Rust code must be formatted with rustfmt:

```bash
make fmt
```

This runs `cargo fmt --all`. The pre-commit hook (installed via `make setup`) enforces this automatically.

### Linting

All code must pass Clippy with warnings treated as errors:

```bash
make lint
```

This runs `cargo clippy -- -D warnings`.

### Testing

oproxy has three test suites:

#### Rust Unit and Integration Tests

```bash
make test-rust
```

This runs `RUSTFLAGS="-D warnings" cargo test --all-features`, which:
- Treats all warnings as errors
- Tests all feature flags including `http3` and `otel`

#### Browser Tests (Playwright)

Browser tests require a running oproxy instance. They are located in `tests/browser/`:

```bash
make test-ui
```

This:
1. Builds a debug binary
2. Installs Playwright dependencies
3. Runs the test suite

To run browser tests manually:
```bash
cd tests/browser
yarn install --frozen-lockfile
yarn test
```

Browser tests run against `http://localhost:18080` by default (set via `OPROXY_BASE_URL`).

#### Python E2E Protocol Tests

Protocol-level tests are in `tests/e2e_protocol_test.py` and `tests/proxy_tester.py`. These require Python and test protocol compliance.

### Full Pre-Release Check

Run all checks before submitting a PR:

```bash
make check
```

This runs: `fmt` + `lint` + `test` (all test suites).

## Project Structure

```
oproxy/
├── src/
│   ├── main.rs                    # Tokio entry point
│   ├── lib.rs                     # Library exports
│   ├── core/                      # ProxyEngine: request lifecycle
│   ├── transport/                 # CONNECT, SOCKS5, WebSocket, TLS
│   ├── middleware/                # Middleware trait and plugins
│   ├── control_plane/             # Management UI and API
│   ├── session/                   # Session management
│   ├── certs/                     # CA and certificate generation
│   ├── config/                    # Configuration loading
│   ├── api/                       # REST API handlers
│   ├── runtime/                   # Startup, listeners, shutdown
│   └── design/                    # React UI (separate package)
├── tests/
│   ├── browser/                   # Playwright browser tests
│   ├── e2e_protocol_test.py       # Python protocol tests
│   └── fixtures/                  # Test fixtures
├── configs/
│   └── default.yaml               # Default configuration
├── docs/                          # Documentation
└── scripts/                       # Utility scripts
```

### UI Development

The React UI lives in `src/design/` and is built separately:

```bash
cd src/design
yarn install --frozen-lockfile
yarn build
```

The build produces `dist/index.html`, `dist/assets/app.js`, and `dist/assets/app.css`, which are embedded into the Rust binary via `build.rs`.

### Adding Middleware

New traffic manipulation features are added by implementing the `Middleware` trait:

```rust
#[async_trait]
pub trait Middleware: Send + Sync {
    fn name(&self) -> &str;
    async fn on_request(&self, ctx: &mut RequestContext) -> MiddlewareAction;
    async fn on_response(&self, ctx: &mut ResponseContext) -> MiddlewareAction;
}
```

See `src/middleware/mod.rs` for the trait definition and existing plugins in `src/middleware/plugins/`.

## Code Style

### Rust

- **Formatting**: Use `cargo fmt --all` (rustfmt)
- **Linting**: Code must pass `cargo clippy -- -D warnings`
- **Warnings**: All warnings are treated as errors in CI and tests
- **Documentation**: Document public APIs with `///` doc comments
- **Error handling**: Use `thiserror` for error types, `?` for propagation
- **Async**: Use `tokio` runtime and `async-trait` for async traits

### JavaScript/TypeScript (UI)

The UI in `src/design/` uses:
- React 19
- Vite for building
- ES modules (`"type": "module"`)
- Yarn 4.15.0 (Berry) with `--immutable` flag

### Commit Messages

Write clear, descriptive commit messages:
- **Prefix**: `feat:`, `fix:`, `docs:`, `chore:`, `refactor:`, `test:`
- **Body**: Explain *why*, not *what* (the diff shows what)

Example:
```
feat: add HTTP/3 QUIC listener support

Implement HTTP/3 protocol support using quinn and h3 crates.
This enables testing HTTP/3 traffic through the proxy.

Closes #123
```

## Pull Request Process

1. **Fork** the repository and create a feature branch from `master`
2. **Make your changes** following the code style guidelines
3. **Add tests** for new functionality
4. **Run all checks**: `make check`
5. **Update documentation** if needed
6. **Submit a pull request** with a clear description

### PR Checklist

Before submitting:

- [ ] Code is formatted (`make fmt`)
- [ ] Code passes Clippy (`make lint`)
- [ ] All tests pass (`make test`)
- [ ] New functionality includes tests
- [ ] Documentation is updated
- [ ] Commit messages are clear and descriptive

### CI Checks

Pull requests must pass:

- `cargo fmt -- --check`
- `cargo clippy --all-targets -- -D warnings`
- `RUSTFLAGS="-D warnings" cargo test --all-targets`
- `cargo audit` (security audit for dependencies)

Browser tests are currently optional in CI (commented out) but recommended to run locally.

## Feature Flags

oproxy uses Cargo feature flags for optional functionality:

- **`http3`**: HTTP/3 (QUIC) support using quinn and h3
- **`otel`**: OpenTelemetry export for protocol spans

Build with features:
```bash
cargo build --release --features http3
```

Or test all features:
```bash
cargo test --all-features
```

## Configuration

Configuration is loaded from (in priority order):

1. Environment variables (`OPROXY_PORT`, `OPROXY_BIND_HOST`, etc.)
2. YAML config file (`OPROXY_CONFIG` env var or `./configs/default.yaml`)
3. Built-in defaults

See `configs/default.yaml` for all available options.

## Documentation

Documentation lives in `docs/` and covers:
- [Getting started](docs/getting-started.md)
- [Docker usage](docs/docker.md)
- [HTTPS MITM](docs/https-mitm.md)
- [Configuration](docs/configuration.md)
- [Security](docs/security.md)
- [Troubleshooting](docs/troubleshooting.md)

Update relevant docs when adding features or changing behavior.

## Security

**Do not open issues for security vulnerabilities.** See [SECURITY.md](docs/security.md) for responsible disclosure guidelines.

## Questions?

- **Bug reports**: [GitHub Issues](https://github.com/sauravrao637/oproxy/issues/new?template=bug_report.yml)
- **Feature requests**: [GitHub Issues](https://github.com/sauravrao637/oproxy/issues/new?template=feature_request.yml)
- **Questions**: [GitHub Discussions](https://github.com/sauravrao637/oproxy/discussions)

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](LICENSE).
