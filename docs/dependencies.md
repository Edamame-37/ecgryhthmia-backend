# Dependency triage

Review date: 2026-10-27. Re-review by 2026-11-27 and whenever crypto/TLS/SQLx features change.

Upgraded SQLx to 0.8.6, jsonwebtoken to 10.4 with AWS-LC verification, Tokio >=1.53.3, current rustls 0.23.46/webpki 0.103.16. MQTT uses native TLS to avoid rumqttc's obsolete optional webpki/pemfile implementation. Keep the host CA store, OpenSSL/Schannel and OS patched; native TLS still performs certificate and hostname validation.

The RustCrypto crypto-bigint and active RSA JWT paths were removed. Runtime allows only ES256/RS256 and optional HS256. ES256 signing in tests also uses AWS-LC.

Exceptions:
- RUSTSEC-2023-0071 (rsa 0.9.10): retained in Cargo.lock by SQLx's optional MySQL graph. MySQL is disabled. cargo tree -i rsa --target all prints no active path. JWT verification uses AWS-LC. Narrow exception in .cargo/audit.toml; enabling MySQL/RustCrypto invalidates it. No vulnerable private-key RSA operation exists in the active application graph.
- GHSA-qwgh-2vcv-g2f7 (block-buffer 0.10.x, OSV): requires catching a panic in a compression callback and reusing that same corrupted buffer. Release profile aborts on panic; application supplies no custom compression callback and has no catch_unwind path. No exploit demonstrated in the active graph. Transitive hash consumers constrain the older digest API. Re-evaluate on upstream digest/SQLx/Axum upgrades; do not suppress unrelated advisories.

The checked scan snapshot is dependency-scan.json. Cargo audit and OSV findings have different scopes/alias behavior; neither scanner alone proves exploitability or clinical correctness. Frontend npm audit is run on full and production graphs; obfuscator/Puppeteer were removed and lockfile refreshed without force upgrades.
