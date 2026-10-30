# Local verification — 30 October 2026

The 2.0.0-rc.1 refinement was verified only against a disposable PostgreSQL service bound to 127.0.0.1:55432 and database ecg_test. No VPS file, production database, Auth account, broker, or deployed process was changed.

Results:

- 39 Rust tests passed with zero ignored.
- cargo fmt --all -- --check passed.
- Clippy for all locked targets with warnings denied passed.
- Three release rollback/readiness fixture tests passed.
- cargo audit --deny warnings passed.
- A fresh isolated schema migrated to 2026102710; the read-only check_db report returned ready=true, acquisition_ready=true, zero unmapped devices, and no issues. The verification schema was then dropped.

The regressions cover reviewed hardware identity mapping, preservation of omitted mappings on edit, duplicate/cross-namespace ambiguity, unmapped ingestion rejection, legacy upgrade without guessed mapping, metadata readiness beyond the migration version, forbidden password-hash drift, RLS drift, and read-only legacy inventory.

Production gates remain external: verified backup/restore rehearsal, coordinated migrations 2026102709/2026102710, manual mapping of all historical hardware IDs, dedicated runtime role, firmware START/STOP ACK and frame sequencing, real MQTT TLS, Auth email/reset, physical ECG calibration, and deployed HTTPS/Nginx acceptance.
