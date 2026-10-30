#!/usr/bin/env bash
set -Eeuo pipefail
: "${TEST_DATABASE_URL:?Set a loopback ecg_test database URL}"
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --locked --release
