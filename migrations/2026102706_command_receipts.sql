CREATE TABLE command_receipts (
 actor_id TEXT NOT NULL REFERENCES accounts(id),
 request_key TEXT NOT NULL,
 signature TEXT NOT NULL,
 session_id TEXT NOT NULL REFERENCES sessions(id),
 PRIMARY KEY(actor_id,request_key)
);
ALTER TABLE command_receipts ENABLE ROW LEVEL SECURITY;
REVOKE ALL ON command_receipts FROM PUBLIC;
DO $$ DECLARE client_role TEXT; BEGIN
 FOREACH client_role IN ARRAY ARRAY['anon','authenticated'] LOOP
  IF EXISTS(SELECT 1 FROM pg_roles WHERE rolname=client_role) THEN
   EXECUTE format('REVOKE ALL ON command_receipts FROM %I',client_role);
  END IF;
 END LOOP;
END $$;
