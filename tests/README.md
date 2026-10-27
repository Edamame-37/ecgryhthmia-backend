# Backend integration tests

The backend uses PostgreSQL and Supabase authentication. Supabase handles login;
the backend registers medical profiles at `POST /api/auth/register_profile`
and reads token claims at `GET /api/auth/me`.

Run the authentication and pacer tests with:

```powershell
cargo test --test integration_tests
```

These tests do not connect to a database at runtime. Compiling the application
still requires `DATABASE_URL` for SQLx query validation, or a prepared `.sqlx`
cache when using `SQLX_OFFLINE=true`.

The three persistence tests are ignored by default. Set `TEST_DATABASE_URL` to a
dedicated PostgreSQL test database to run them:

```powershell
$env:TEST_DATABASE_URL = 'postgres://postgres:postgres@127.0.0.1:5432/ecg_test'
cargo test --test integration_tests -- --include-ignored
```

The test user needs permission to create schemas. Each persistence test creates
a unique `ecg_test_<uuid>` schema, runs the application's migrations, and drops
that schema after successful completion. Runtime writes never fall back to
`DATABASE_URL`. If a test panics, its schema may remain for inspection. The worker
test also creates a unique `records/ses_test_*.jsonl` file and removes it after
successful completion.
