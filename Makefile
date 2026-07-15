.PHONY: help setup fmt build build-release ui check-dist test lint clean soak-test

# ── Defaults ──────────────────────────────────────────────────────────────────

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) \
		| awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-18s\033[0m %s\n", $$1, $$2}'

# ── Dev setup ─────────────────────────────────────────────────────────────────

setup: ## Verify required tooling, then install git hooks (pre-commit: cargo fmt --all)
	cargo xtask setup

# ── Formatting ────────────────────────────────────────────────────────────────

fmt: ## Format all Rust code (cargo fmt --all)
	cargo fmt --all

# ── Build ─────────────────────────────────────────────────────────────────────

build: ## Debug build
	cargo build

build-release: ## Release build (includes UI assets)
	cargo build --release

ui: ## Build the React UI assets (requires Node + Yarn)
	cargo xtask build-ui

check-dist: ## Verify src/design/dist exists and has no dev-only/CDN leakage
	cargo xtask check-dist

audit:
	cargo audit

# ── Quality ───────────────────────────────────────────────────────────────────

test: test-rust test-ui ## Run all tests (Rust + Playwright browser tests)

test-rust: export RUSTFLAGS := -D warnings
test-rust: ## Run Rust unit/integration tests
	cargo test --all-features

test-ui: ## Run Playwright browser tests (builds debug binary first)
	@echo "Building debug binary for browser tests..."
	cargo build
	@echo "Running Playwright tests..."
	yarn --cwd tests/browser install --immutable
	yarn --cwd tests/browser test

lint: ## Run Clippy (warnings as errors)
	cargo clippy --all-targets -- -D warnings

check: fmt lint test audit## fmt + lint + test (full pre-release check)

soak-test: build-release ## Baseline latency + sustained-load/memory-ceiling check (see docs/performance.md)
	bash scripts/soak-test.sh

# ── Housekeeping ──────────────────────────────────────────────────────────────

clean: ## Remove build artifacts
	cargo clean
