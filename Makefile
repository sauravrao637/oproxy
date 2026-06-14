.PHONY: help setup fmt build build-release test lint clean

# ── Defaults ──────────────────────────────────────────────────────────────────

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) \
		| awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-18s\033[0m %s\n", $$1, $$2}'

# ── Dev setup ─────────────────────────────────────────────────────────────────

setup: ## Install git hooks (pre-commit: cargo fmt --all)
	@mkdir -p .git/hooks
	@printf '#!/usr/bin/env sh\nset -e\necho "Running cargo fmt --all..."\ncargo fmt --all -- --check\n' > .git/hooks/pre-commit
	@chmod +x .git/hooks/pre-commit
	@echo "✓ Pre-commit hook installed (.git/hooks/pre-commit)"
	@echo "  Runs: cargo fmt --all -- --check"
	@echo "  Fix:  make fmt"

# ── Formatting ────────────────────────────────────────────────────────────────

fmt: ## Format all Rust code (cargo fmt --all)
	cargo fmt --all

# ── Build ─────────────────────────────────────────────────────────────────────

build: ## Debug build
	cargo build

build-release: ## Release build (includes UI assets)
	cargo build --release

ui: ## Build the React UI assets (requires Node + Yarn)
	corepack enable
	yarn --cwd src/design install --immutable
	yarn --cwd src/design build

# ── Quality ───────────────────────────────────────────────────────────────────

test: test-rust test-ui ## Run all tests (Rust + Playwright browser tests)

test-rust: ## Run Rust unit/integration tests
	RUSTFLAGS="-D warnings" cargo test --all-features

test-ui: ## Run Playwright browser tests (builds debug binary first)
	@echo "Building debug binary for browser tests..."
	cargo build
	@echo "Running Playwright tests..."
	yarn --cwd tests/browser install --immutable
	yarn --cwd tests/browser test

lint: ## Run Clippy (warnings as errors)
	cargo clippy --all-targets -- -D warnings

check: fmt lint test ## fmt + lint + test (full pre-release check)

# ── Housekeeping ──────────────────────────────────────────────────────────────

clean: ## Remove build artifacts
	cargo clean
