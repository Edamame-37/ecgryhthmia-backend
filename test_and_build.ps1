$ErrorActionPreference = 'Stop'
if (-not $env:TEST_DATABASE_URL) { throw 'TEST_DATABASE_URL is required; never use DATABASE_URL for tests' }
cargo fmt --all -- --check
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
cargo clippy --locked --all-targets -- -D warnings
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
cargo test --locked --all-targets
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
cargo build --locked --release
exit $LASTEXITCODE
