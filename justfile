# compositor — task runner
#
# Public/shared repo: `default` is self-contained `@just --list`. For a private
# repo this would be `@"$HOME/.scripts/just-pretty.sh"` (see lockyc-config skill).

# List available recipes
default:
    @just --list

# Build the workspace
[group("build")]
build:
    cargo build

# Run the CLI (e.g. `just run build --dir .`)
[group("build")]
run *args:
    cargo run -p compositor -- {{args}}

# Run the test suite
[group("check")]
test:
    cargo test

# Compile without producing binaries
[group("check")]
check:
    cargo check --all-targets

# Format all Rust files in place
[group("check")]
fmt:
    cargo fmt

# Clippy lints (warnings are errors)
[group("check")]
lint:
    cargo clippy --all-targets -- -D warnings

# Non-mutating pre-merge gate: fmt check + clippy + tests
[group("check")]
gate:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings
    cargo test
    echo "✓ gate passed"
