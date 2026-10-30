CREATE TABLE file_cleanup (
 id BIGSERIAL PRIMARY KEY,
 root_kind TEXT NOT NULL CHECK(root_kind IN ('records','uploads')),
 path TEXT NOT NULL UNIQUE,
 not_before TIMESTAMPTZ NOT NULL DEFAULT now(),
 attempts INTEGER NOT NULL DEFAULT 0
);
ALTER TABLE file_cleanup ENABLE ROW LEVEL SECURITY;
REVOKE ALL ON file_cleanup FROM PUBLIC;
DO $$ DECLARE client_role TEXT; BEGIN
 FOREACH client_role IN ARRAY ARRAY['anon','authenticated'] LOOP
  IF EXISTS(SELECT 1 FROM pg_roles WHERE rolname=client_role) THEN
   EXECUTE format('REVOKE ALL ON file_cleanup FROM %I',client_role);
  END IF;
 END LOOP;
END $$;
