# Test safety and coverage

Set TEST_DATABASE_URL to a disposable PostgreSQL database named ecg_test on localhost/127.0.0.1/::1. DATABASE_URL is never a fallback. Each test creates its own random schema, runs all migrations, creates synthetic identities with distinct Auth/account/profile IDs, and removes only that schema. Fixture storage uses TempDir. The guard rejects unknown targets before connecting.

Coverage includes:
- Dynamically generated ES256/JWKS: valid, expired, tampered, bad issuer/audience/iat, unknown key; cache TTL/key removal.
- Anonymous and patient/assigned doctor/unrelated doctor/admin API permissions, role metadata escalation, account revocation and token cutoff.
- Supabase profile provisioning transaction/FK ordering, retries, unauthorized role/email, actor-bound impersonation and audit.
- One-use expiring pairing, scoped recording reads/exports, traversal and stored checksum corruption.
- START/STOP idempotency and firmware ACK transitions, replay/source/session/sequence checks, restart continuity.
- CSV Lead I/II/III mapping and malformed samples/time values.
- Real loopback WebSocket handshakes, Origin, ticket reuse, two patient scopes and account revocation; independent device pacers.
- Orphan cleanup preserves linked files, removes unreferenced files and rejects paths outside storage.

Tests inject MQTT client queues and ACKs. They do not contact a physical broker, Supabase project or patient device. Hardware/TLS/provisioning delivery acceptance remains a staging exercise. Browser fixtures are separately documented in the frontend README.

No test is ignored and no DB scenario returns early when configuration is absent.
