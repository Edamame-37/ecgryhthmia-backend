# Atomic commit commands

Prepared only; no git add, commit or push has been executed. Run from this repository root after reviewing the complete diff. The first commit keeps the interdependent API/schema/client changes and their regression tests together.

## 1. fix(security): enforce scoped ECG acquisition and review

```powershell
git add -- '.cargo/audit.toml' '.env.example' '.gitattributes' '.gitignore' 'Cargo.lock' 'Cargo.toml' 'diff_with_main.txt' 'migrations/2026102701_baseline.sql' 'migrations/2026102702_identity_and_access.sql' 'migrations/2026102703_recording_integrity.sql' 'migrations/2026102704_patient_birth_date.sql' 'migrations/2026102705_operational_integrity.sql' 'migrations/2026102706_command_receipts.sql' 'migrations/2026102707_file_cleanup.sql' 'migrations/2026102708_device_revision.sql' 'src/api/accounts.rs' 'src/api/auth.rs' 'src/api/authorization.rs' 'src/api/devices.rs' 'src/api/error.rs' 'src/api/impersonation.rs' 'src/api/jwks.rs' 'src/api/limits.rs' 'src/api/mod.rs' 'src/api/patients.rs' 'src/api/recordings.rs' 'src/api/routes.rs' 'src/api/stream.rs' 'src/bin/check_db.rs' 'src/bin/check_sessions.rs' 'src/bin/reconcile.rs' 'src/config.rs' 'src/data/csv_reader.rs' 'src/data/mod.rs' 'src/db/postgres.rs' 'src/db/legacy_migrations.json' 'src/lib.rs' 'src/main.rs' 'src/models/device.rs' 'src/models/mod.rs' 'src/models/payload.rs' 'src/network/mod.rs' 'src/network/mqtt_listener.rs' 'src/network/pacer.rs' 'src/network/websocket.rs' 'src/services/ingestion.rs' 'src/services/mod.rs' 'src/services/recording.rs' 'src/services/storage.rs' 'tests/account_tests.rs' 'tests/auth_tests.rs' 'tests/ingestion_tests.rs' 'tests/integration_tests.rs' 'tests/migration_tests.rs' 'tests/recording_tests.rs' 'tests/security_tests.rs' 'tests/stream_tests.rs' 'tests/support/mod.rs' 'tests/upload_tests.rs'
git commit -m "fix(security): enforce scoped ECG acquisition and review"
```

## 2. ci: gate immutable releases with readiness and rollback checks

```powershell
git add -- '.github/workflows/ci.yml' 'deploy/database-role.sql' 'deploy/ecg-backend.service' 'deploy/nginx.conf' 'deploy/release.sh' 'test_and_build.ps1' 'test_and_build.sh' 'tests/deployment_tests.py'
git commit -m "ci: gate immutable releases with readiness and rollback checks"
```

## 3. docs: record audit remediation and rollout evidence

```powershell
git add -- 'README.md' 'docs/audit-remediation.md' 'docs/commit-commands.md' 'docs/contracts.md' 'docs/dependencies.md' 'docs/dependency-scan.json' 'docs/evidence/cargo-audit.log' 'docs/evidence/clippy.log' 'docs/evidence/deployment-tests.log' 'docs/evidence/release-build.log' 'docs/evidence/rust-tests.log' 'docs/rollout.md' 'docs/schema-upgrade.md' 'docs/evidence/startup-checks.json' 'docs/verification.json' 'docs/verification.md' 'tests/README.md'
git commit -m "docs: record audit remediation and rollout evidence"
```

## MQTT allowlist configuration follow-up

The approved local fix adds the already configured HiveMQ hostname to the ignored backend `.env`. Do not stage `.env` or force-add credentials. If the existing working-tree changes above have not been committed, include the MQTT documentation in the documentation commit above. If they have already been committed, use this separate documentation commit:

```powershell
git add -- 'README.md' 'docs/commit-commands.md'
git commit -m "docs(mqtt): explain broker allowlist configuration"
```

## Legacy private images and database error follow-up

The photo compatibility fix and its regression tests are included in the application paths in commit 1 above. If that pending audit baseline has not been committed, review them together there and include the refreshed evidence in commit 3. Do not stage the same changes twice. After that baseline already exists, use these separate atomic commits for this follow-up:

```powershell
git add -- 'src/api/error.rs' 'src/api/recordings.rs' 'tests/security_tests.rs'
git commit -m "fix(api): resolve legacy private images and classify database errors"
```

```powershell
git add -- 'docs/verification.md' 'docs/verification.json' 'docs/evidence/rust-tests.log' 'docs/evidence/clippy.log' 'docs/commit-commands.md'
git commit -m "docs: record private image compatibility verification"
```

The configured database was inspected read-only for this follow-up. No migration, account update, file-reference rewrite, Git staging, commit or push was performed for it.
