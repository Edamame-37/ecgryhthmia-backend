# Audit remediation ledger

Basis: ECGRHYTHMIA_AUDIT.md (25 October 2026), applied to the two local repositories, not directly to the different deployed commits cited by the audit. Initial implementation and verification: 27 October 2026. Startup/schema upgrade verification: 28 October 2026.

All 40 findings have source/configuration/test remediation in this change. **Production findings are not declared closed.** Deployment, real Supabase/Auth delivery, database role/grant inspection, real broker/firmware, physical calibration, credential rotation and production historical-data repair remain explicit rollout acceptance tasks. See [rollout.md](rollout.md). Local tests use isolated databases. The separately approved configured-database upgrade and its preservation checks are recorded in [schema-upgrade.md](schema-upgrade.md); no remote application deployment or Auth changes were performed.

Evidence labels: **DB** = disposable loopback PostgreSQL; **WS** = actual loopback WebSocket; **Browser** = Chromium with synthetic Auth/API fixtures; **Unit** = isolated logic/component test; **Config** = prepared configuration, not a live-host claim.

| Finding | Implemented remediation | Local evidence and remaining boundary |
|---|---|---|
| SEC-01 | RLS, revoke anon/authenticated/PUBLIC private grants, isolate MQTT credentials, remove legacy hashes, service-only DB role policy template; FE private access through API | DB migration/grants tests and legacy-upgrade test. Production PostgREST/RPC inheritance and dedicated-role rollout pending. |
| SEC-02 | Private router authentication by default; database roles and resource ownership on every current private handler | DB anonymous/cross-patient/assigned doctor/admin matrix. Deployed legacy routes must be replaced at cutover. |
| SEC-03 | One-use ticket, exact Origin, session routing, socket reauthorization and bounded client counts | WS scope/Origin/reuse/revocation tests and independent device pacer. Real proxy/broker validation pending. |
| SEC-04 | User-editable metadata never supplies permissions; normalized DB role allowlist | DB signed metadata escalation test; Browser overrides stored role with server identity. |
| SEC-05 | Removed local password-login/hash comparison; all login/reset through Supabase Auth; legacy hash column removed | Legacy login route 404 and migration test. Real Auth login/reset/password delivery and exposure response pending. |
| SEC-06 | Validated IDs, DB-derived paths, canonical storage confinement for reads/deletes | DB traversal/outside-file/corrupt-storage tests. Legacy relocation requires explicit review. |
| SEC-07 | iat/exp/issuer/audience checks, 60s JWKS TTL, Auth session/account cutoff, repeated socket JWT and scope checks | Dynamic ES256 tests, key removal TTL, DB revocation/cutoff, WS account revocation. Real Supabase session/signing-key rotation pending. |
| SEC-08 | 8KiB ordinary bodies, bounded heavy bodies/concurrency, per-actor rates, message/sample/duration limits, allowlisted TLS brokers/topics, pagination/timeouts | DB rate/validation tests; bounded workers/socket permits/pacer; Config gateway limits. Load acceptance on target host pending. |
| SEC-09 | Exact serialized UTF-8 content envelope and SHA256 verification before parsing/use | FE checksum/tamper test and BE stored integrity failure. TLS remains transport authentication. |
| ECG-01 | Reference mV coordinates; gain 5/10/20mm/mV applied once in common canvas | Unit physical pulse/gain tests; screen ruler calibration remains manual. |
| ECG-02 | aVR computed as -(I+II)/2, no second SVG inversion | Unit polarity and marker-transform tests. |
| ECG-03 | Shared history/live reference time coordinates; chosen speed applied once | Unit 12.5/25/50mm/s/calibration/marker tests. |
| ECG-04 | Exact normalized CSV channel aliases; reject duplicates/missing/nonfinite channels | Rust CSV mapping/error tests, including swapped column order. |
| ECG-05 | Explicit causal second-order HP 0.5Hz and LP 40Hz at actual fs; independent measured channels | Unit DC/pass-band/stop-band tests at 125/250/500Hz. Filter/algorithm is not clinically certified. |
| ECG-06 | Insufficient RR and unknown AI are distinct; either rule anomaly or AI anomaly raises the same warning | Unit clinical engine/component tests; no false Normal from empty data. |
| ECG-07 | Rate/count/duration/frame offset in source, ingestion, stream and shared rendering/export; preserve sub-sample RR precision | Unit variable-rate coordinates, DB persisted times, Browser 125Hz history. Physical device clock/rate acceptance pending. |
| ECG-08 | Missing prediction/telemetry UNKNOWN/unavailable, no fabricated HR/QTc/battery/accuracy; verified fresh stream indicator | Unit unknown/insufficient cases; removed unsupported landing-page claims. No model-accuracy claim. |
| DATA-01 | Upload validation and source ordering; duration=count/fs; accumulated start/end/frame numbers rather than fixed 10s buckets | DB ingestion/timing and legacy reconciliation tests. Existing bad metadata is preserved until explicit manifest repair. |
| DATA-02 | Requested/recording/stopping/ended/interrupted state machine, request-bound idempotency receipts, final-frame ACK watermark | DB command/ACK/retry tests; UI server-state polling. Firmware change and broker delivery acceptance pending. |
| DATA-03 | UUID IDs and DB-locked monotonic per-session counters survive process restart | DB replay/restart continuity tests. No global in-memory ID/sequence reset. |
| DATA-04 | Listener-owned internal device, hardware name/session/source-message/sequence/timestamp validation; replace old listener task | DB wrong listener/session/spoof/stale/replay tests. Real broker ACL/topic ownership pending. |
| DATA-05 | Per-device bounded pacing, global in-flight cap, bounded ingress and browser verification/worker queues; socket/task cleanup | WS two-device latency, receiver cleanup, socket permit and FE worker coalescing tests. Target-device load measurements pending. |
| DATA-06 | Immutable checksummed frame files, atomic rename+DB metadata, tombstones, delayed orphan cleanup and retry queue; backed-up explicit legacy repair | DB corrupt/read/hide/delete/cleanup tests and real reconcile CLI dry-run/apply fixture. Production manifests/backup/repair pending. |
| DATA-07 | Account-first transactional profile provisioning with required role profile fields and explicit external Auth compensation | DB provisioning/FK/retry/invalid role/email tests. Real Auth admin delivery failure recovery pending. |
| DATA-08 | One versioned backend migration source, readiness schema gate, no frontend direct schema/DB writes | Fresh DB and legacy-copy migration tests; incompatible deployed histories must be reviewed before cutover. |
| INT-01 | Signup-confirm-login provisioning through verified subject/email; pending profile retries and explicit existing-session recovery | DB provisioning and authenticated Browser fixture flows. Real email confirmation/redirect/password reset pending. |
| INT-02 | Resource-bound expiring one-use consent/device codes; await server persistence before success | DB pairing/expired/reused code tests. Physical camera and broker pairing staging pending. |
| INT-03 | Shared API client/errors/types, current endpoints, real monitor/reset routes, explicit 404 for obsolete no-op routes | Typecheck, Browser all three roles, DB API tests. No SQLite/token fallback. |
| INT-04 | Explicit Auth/account/profile IDs; me and scoped server endpoints; header/profile use normalized identity | DB distinct-ID fixtures and Browser fixtures with different IDs. Production identity mapping pending. |
| INT-05 | Server-issued actor-bound expiring impersonation, current effective permissions, audit and cache partition; stop resolves original actor | DB impersonation/cross-actor/revocation tests. Live-host cache/session exercise pending. |
| INT-06 | Ticket/reconnect backoff; committed latest-frame recovery; absolute frame-number navigation; independent live/history snapshots; actual-rate playback | FE hook/component tests and Browser history; WS real session subscription. Firmware outage recovery pending. |
| INT-07 | Persist DOB date and derive age on server/display; correct first/last-name payloads and unknown DOB | Typecheck, DB DOB provisioning, profile implementation. Legacy DOB remains unknown rather than fabricated. |
| INT-08 | Truthful server errors, requested/ACK status, telemetry freshness, actual counters, safe camera lifecycle/manual fallback; remove fake DevTools security | Browser monitor states and Unit rejected command; physical camera/device indicators pending. |
| OPS-01 | PR/push checks, immutable same-SHA artifacts, pinned SSH keys, fail-hard readiness, atomic symlink and rollback scripts | Release-script success/failure fixture tests; actual staging deployment/rollback still pending. |
| OPS-02 | Dedicated non-root systemd user, protected env, fixed writable data paths, resource limits; remove tracked credential literals | Config and targeted current-tree scan. Actual host permissions and previously exposed credential rotation pending; history not rewritten. |
| OPS-03 | Exact-project CSP and browser security headers, private no-store API/images, camera policy, no WS ticket query logs | Config/shell syntax plus Browser functional tests. HTTPS/Nginx header/PWA/camera enforcement must be checked on staging. |
| OPS-04 | Controlled lockfile upgrades, remove unused obfuscation/Puppeteer, supported JWT/TLS paths, narrow dated advisory exceptions | npm full/production audit; cargo audit and OSV snapshot. See dependencies.md for two reviewed graph/conditional exceptions. |
| QA-01 | Mandatory TEST_DATABASE_URL loopback/ecg_test guard before connecting; random per-test schemas and TempDir storage | Guard rejects production/unknown targets; DB suite runs in parallel without deleting other fixture schemas. |
| QA-02 | Actual tsc -b on PR, required disposable DB, dynamic ES256 tokens, role/command/stream/provision/repair tests; no hidden early-return passes | FE unit/Browser and BE all-target DB suites. Synthetic tests are labeled separately from real external integrations. |
| QA-03 | Lazy routes/chunks, safe persistent JSON, error boundary before providers, remove dead alternate pipelines; strict lint and bounded worker processing | Build bundle measurement, lint/typecheck, invalid-storage and worker coalescing tests. Target mobile performance acceptance pending. |

## Local acceptance record

Final command results and exact counts are recorded in verification.md. No test suite can substitute for the staging/production items in the last column.

~~~mermaid
flowchart LR
 Browser[React + Supabase SDK] -->|login / reset| Auth[Supabase Auth]
 Browser -->|JWT + optional actor context| API[Private Rust API]
 API -->|DB identity and ownership| DB[(PostgreSQL + RLS)]
 Browser -->|one-use session ticket| WS[Scoped WebSocket]
 Device[Device + exact TLS topic] --> Ingress[Bounded ingestion]
 Ingress --> Files[Immutable frame files]
 Ingress --> DB
 DB --> Cleanup[Orphan/deletion reconciliation]
 Ingress --> Pacer[Bounded per-device pacing]
 Pacer --> WS
 WS --> Verify[Checksum + bounded worker queue]
 Verify --> Canvas[Shared ECG coordinates]
~~~
