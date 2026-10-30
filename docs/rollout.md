# Staging and production rollout

## 2.0.0-rc.1 gates

1. Run `check_db` read-only against an isolated restore before and after explicit migrations `2026102709` and `2026102710`. API readiness requires `ready=true`; acquisition additionally requires `acquisition_ready=true`.
2. Review every nullable `devices.hardware_id` against firmware inventory. Never infer it from a display name. Keep acquisition stopped until all intended devices are mapped.
3. Run `cargo run --bin reconcile -- --inventory` for a read-only aggregate of missing, outside-root, oversized, ambiguous, and unmapped legacy sources before preparing any per-session manifest.
4. Use the checked-in domains `ecgrhythmia.cloud`, `www.ecgrhythmia.cloud`, `api.ecgrhythmia.cloud`, and the reviewed Supabase project. Production runtime must have `APP_ENV=production`, explicit origins/brokers/absolute storage paths, and `APPLY_MIGRATIONS=false`.
5. Migration `2026102710` removes a reintroduced legacy `accounts.password_hash` idempotently. Confirm the old password-login binary is retired and a verified backup exists before cutover.

The configured Supabase database received the explicitly approved startup/schema upgrade on 28 October 2026; see [schema-upgrade.md](schema-upgrade.md) for backup and preservation evidence. Remote application deployment and the remaining runtime acceptance checks are separate and pending. Local fixtures are not proof of a production cutover.

## Before migration

1. Pin both repository commit SHAs. Read audit-remediation.md and contracts.md. Inventory the deployed schema, _sqlx_migrations, policies/grants, security-definer functions and PostgREST RPCs. The original audit examined different deployed commits; never assume local migrations match that history.
2. Make and verify a PostgreSQL backup and records/uploads backup. Test restoring both together to an isolated staging project. Protect backups as clinical data; restrict access and retain them according to the actual policy.
3. Compare the deployed schema with migrations/2026102701_baseline.sql. IF NOT EXISTS does not reconcile incompatible column types/constraints. Resolve incompatible old migration histories explicitly on the staging copy; never delete a migration ledger just to bypass a checksum/version error.
4. Check duplicate account/profile/device assignments, null identities, auth.users-to-accounts mappings and conflicting recording states. Migration backfills auth_id=id only for legacy IDs; confirm that mapping against Auth. Use verified mapping updates where IDs differ, never infer an account by display name.
5. Stop acquisition and writes during cutover. The versioned migrations separate MQTT credentials, drop legacy password_hash and credential columns, add RLS/revoke browser grants, DOB, recording integrity and cleanup receipts. They can intentionally fail on conflicting unique assignments. Existing duplicate patient profiles are preserved; login and impersonation reject ambiguous accounts, and a trigger prevents new duplicate assignments. Investigate other conflicts; do not bypass constraints.
6. Configure Supabase JWT expiry <=1 hour, email confirmation, exact redirect allowlist (/auth/login and /auth/reset-password?mode=update), password policy and supported signing keys. Rotate the formerly hardcoded DB credential and exposed MQTT/other secrets through the service owners. Git history still contains removed literals; removing a file does not rotate a secret. Evaluate account reset/session revocation based on actual exposure.
7. Set backend env outside the release directory (root:ecgrhythmia 0640 or owner-only 0600 as appropriate). Frontend env contains public URL/anon key only. SUPABASE_SERVICE_ROLE_KEY and optional SUPABASE_JWT_SECRET remain backend-only.

## Stage the new release

1. Run migrations explicitly with a privileged migration identity on the staging copy.
2. Provision a non-superuser/non-BYPASSRLS database login and apply deploy/database-role.sql with the migration owner; set DATABASE_URL to that login for runtime. Its private-data policy is service-only; the API enforces patient/doctor/admin ownership. Grant only the stated auth.sessions columns for revocation checks. It cannot run migrations or assign browser roles.
3. Independently verify anon/authenticated cannot SELECT credentials/accounts or write private tables through PostgREST, and cannot use a SECURITY DEFINER RPC to bypass restrictions. Revoke obsolete RPC execution if the deployed project has such functions. Confirm no browser role inherits the backend role.
4. Replay the role matrix, signup-confirm-login/profile retry, password reset, patient consent and device pairing, unauthorized API/WS, account/session/key revocation and impersonation tests against staging identities.
5. Update firmware to contracts.md before acquisition: server session IDs, monotonic per-session frame numbers, source message deduplication, START/STOP ACK and final-frame watermark. Test real broker TLS success and invalid certificate/hostname rejection. Test simultaneous devices, network loss, reconnect/restart, missing/duplicate frames, queue backpressure and final frame arrival.
6. Run reconcile dry-run for legacy sessions. Prepare explicit frame-to-source-line manifests for ambiguous/duplicate historical data, inspect backups and dry-run output, then apply to the staging copy. Compare counts, source hashes, original frame IDs, review/hide/delete fields and times. Never repair ambiguous history by guessing.
7. Check new-file cleanup and deletion retry under interrupted writes/permission failures. Cleanup reservations protect writes abandoned before DB commit; canonicalization rejects outside paths. Existing legacy files outside the configured root require a reviewed relocation.
8. Validate physical ECG calibration with a ruler on target devices; synthetic impulse/sine/QRS tests do not establish diagnostic accuracy. Verify 125/250/500Hz, gain/speed combinations, marker alignment, filters, CSV/JSON exports and hidden-frame timing.
9. Install the dedicated systemd user/unit; create /var/lib/ecgrhythmia/{records,uploads} owned by it. Keep release code read-only. Verify non-root recording writes, resource limits and actual env file permissions.
10. Replace Nginx domain/project placeholders, configure certificates and run nginx -t. Header CSP permits only the selected Auth project and same-origin API/WS. Test camera permission/manual QR fallback, PWA/update behavior, styles/fonts, private images and downloads in a real HTTPS browser. Enable HSTS only after HTTPS readiness. Do not log WS ticket query strings.

## Release and rollback

The CI workflow runs on PR/push and checks types/lint/tests/advisories before packaging the tested SHA. Production deployment is a manual workflow_dispatch on main and uses that job's artifact. Configure GitHub variables VITE_* (frontend), DEPLOY_HOST/DEPLOY_USER/DEPLOY_SSH_KEY/DEPLOY_KNOWN_HOSTS (secrets), a protected production environment and required branch checks. Store independently verified host keys; never replace StrictHostKeyChecking with no or trust ssh-keyscan blindly.

Install Python 3, Bash, GNU tar/coreutils, curl and util-linux flock on the deployment host. Install reviewed release.sh outside writable release archives. Create incoming/releases directories owned by the deployment account and grant only the exact systemctl restart command via sudoers. Backend readiness must report the expected compiled BUILD_SHA; frontend version.json must match the tested SHA. Keep releases immutable and record paired FE/BE SHAs.

Test failed startup/readiness on staging and confirm the previous compatible artifact is restored and the deployment reports failure. Binary rollback is valid only between binaries compatible with the migrated schema. The old pre-remediation backend is incompatible: initial schema rollback requires a coordinated maintenance window and restore of the DB/files/Auth mapping backup. Do not claim an old-binary symlink alone rolls back these destructive schema changes.

After production cutover, repeat acceptance checks, inspect rejects/command timeout/cleanup retry logs, and record deployed SHAs plus measured results in the finding ledger. The database upgrade recorded in schema-upgrade.md is the only configured-service change performed here. No remote application deployment, service restart or credential rotation was performed.
