# ECGRhythmia backend

Rust/Axum API, PostgreSQL, authenticated session-scoped WebSockets and MQTT ingestion. Browser clients use Supabase Auth; private database tables are accessed exclusively through this API.

## Setup

Copy .env.example to .env and supply your own connection settings. Never reuse a production URL for tests. Install Rust stable and the native TLS build prerequisites (Linux: pkg-config, libssl-dev and current ca-certificates; Windows: MSVC build tools/CMake). MQTT uses verified native TLS on port 8883; SQLx/HTTP use rustls.

Migrations are explicit:

~~~sh
cargo run --bin ecg-backend -- --migrate-only
cargo run --bin ecg-backend
~~~

Existing deployments require the backup, schema and identity checks in [rollout.md](docs/rollout.md) before migrating. The schema changes are incompatible with old binaries. Do not run an old schema initializer or frontend SQL scripts. The approved upgrade of the currently configured database and the preserved legacy identity conflicts are recorded in [schema-upgrade.md](docs/schema-upgrade.md).

## MQTT configuration

Set `MQTT_ALLOWED_BROKERS` in the backend `.env` to a comma-separated list of trusted broker hostnames, without a URL scheme, port or path. Restart the backend after changing this setting. TLS port 8883 and exact topics are required.

```dotenv
MQTT_ALLOWED_BROKERS=broker.example.org
```

Each device's broker, port, exact frame topic and credentials come from the private `device_credentials` database table. Configure them through the administrator device API. Legacy `.env` settings `MQTT_BROKER`, `MQTT_PORT` and `MQTT_TOPIC` do not configure these listeners. A legacy wildcard such as `ecgrhythmia/#` must not be copied into a device's frame topic; use its exact topic, for example `ecgrhythmia/device01`.

When startup reports `MQTT broker must be allowlisted and use TLS port 8883; wildcard topics are prohibited`, compare the device's database hostname with `MQTT_ALLOWED_BROKERS`, confirm port 8883, and check that its topic contains neither `#` nor `+`. MQTT credentials and the backend `.env` remain excluded from Git.

## Checks

~~~powershell
$env:TEST_DATABASE_URL='postgres://postgres:test-only@127.0.0.1:5432/ecg_test'
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo audit --deny warnings
~~~

Tests require loopback and a database named ecg_test before opening any connection. They create independent schemas and temporary recording directories. Missing TEST_DATABASE_URL is a failure. The database fixture role needs CREATE SCHEMA; disposable test clusters should also contain NOLOGIN roles anon and authenticated.

See [test coverage](tests/README.md), [API/device contracts](docs/contracts.md), [finding status](docs/audit-remediation.md) and [dependency triage](docs/dependencies.md).

## Maintenance

~~~sh
cargo run --bin reconcile
cargo run --bin reconcile -- --session ses_example --manifest mapping.json
cargo run --bin reconcile -- --session ses_example --manifest mapping.json --backup /secure/backups --apply
~~~

A manifest is an ordered array of {"frame_id":"fra_existing","line_number":1}. Line numbers are one-based source lines. Every existing frame must be mapped once. Dry-run is the default. Apply requires a stopped session and writes durable source/metadata/manifest backups first. It preserves IDs, clinical review/hide/delete fields and original source files. Ambiguous mapping must be investigated manually. It does not invent missing waveforms.

New frame/image files have delayed cleanup reservations. Committed references preserve the file; abandoned writes and queued deletions are reconciled by housekeeping. Failures are logged with a cleanup ID and retried. The cleanup worker canonicalizes every deletion path.
