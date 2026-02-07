[private]
default:
  @just --list

# Build debug binary
build:
  cargo build

# Build release binary
release:
  cargo build --release

# Run clippy and check
check:
  cargo check --all-targets
  cargo clippy --all-targets -- -D warnings

# Format code (Rust, Markdown, YAML, Nix)
fmt:
  cargo fmt --all
  dprint fmt
  nixfmt *.nix

# Run tests
test:
  cargo test --all-targets

# Verify (fmt + check + test)
verify: fmt check test

# CI: format check, lint, build, and test
ci:
  cargo fmt --all -- --check
  cargo check --all-targets
  cargo clippy --all-targets -- -D warnings
  cargo build
  cargo test --all-targets

# Clean build artifacts
clean:
  cargo clean
