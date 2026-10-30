ALTER TABLE accounts ADD COLUMN IF NOT EXISTS auth_id TEXT;
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS enabled BOOLEAN NOT NULL DEFAULT true;
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS tokens_valid_after BIGINT NOT NULL DEFAULT 0;
UPDATE accounts SET auth_id=id WHERE auth_id IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS accounts_auth_id_unique ON accounts(auth_id);
CREATE UNIQUE INDEX IF NOT EXISTS doctors_account_unique ON doctors(account_id);
-- Preserve ambiguous legacy profiles instead of merging clinical identities.
CREATE INDEX IF NOT EXISTS patients_account_lookup ON patients(account_id);
CREATE FUNCTION prevent_new_duplicate_patient_profiles() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='UPDATE' THEN
  IF OLD.account_id IS NOT DISTINCT FROM NEW.account_id THEN RETURN NEW; END IF;
 END IF;
 IF NEW.account_id IS NULL THEN RETURN NEW; END IF;
 PERFORM pg_advisory_xact_lock(hashtextextended('patient-profile:'||NEW.account_id,0));
 IF EXISTS(SELECT 1 FROM patients WHERE account_id=NEW.account_id AND id<>NEW.id) THEN
  RAISE EXCEPTION 'Account already has a patient profile' USING ERRCODE='23505';
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER patient_profile_account_guard BEFORE INSERT OR UPDATE OF account_id ON patients FOR EACH ROW EXECUTE FUNCTION prevent_new_duplicate_patient_profiles();
CREATE TABLE device_credentials (device_id TEXT PRIMARY KEY REFERENCES devices(id) ON DELETE CASCADE,mqtt_broker TEXT NOT NULL,mqtt_port INTEGER NOT NULL CHECK(mqtt_port BETWEEN 1 AND 65535),mqtt_topic TEXT NOT NULL,mqtt_username TEXT NOT NULL,mqtt_password TEXT NOT NULL);
INSERT INTO device_credentials SELECT id,mqtt_broker,mqtt_port,mqtt_topic,COALESCE(mqtt_username,''),COALESCE(mqtt_password,'') FROM devices WHERE mqtt_broker IS NOT NULL AND mqtt_port BETWEEN 1 AND 65535 AND mqtt_topic IS NOT NULL;
ALTER TABLE devices DROP COLUMN IF EXISTS mqtt_broker;
ALTER TABLE devices DROP COLUMN IF EXISTS mqtt_port;
ALTER TABLE devices DROP COLUMN IF EXISTS mqtt_topic;
ALTER TABLE devices DROP COLUMN IF EXISTS mqtt_username;
ALTER TABLE devices DROP COLUMN IF EXISTS mqtt_password;
ALTER TABLE devices ADD COLUMN IF NOT EXISTS last_seen_at TIMESTAMPTZ;
CREATE TABLE pairing_codes (digest TEXT PRIMARY KEY,resource_id TEXT NOT NULL,kind TEXT NOT NULL CHECK(kind IN ('patient','device')),issued_by TEXT NOT NULL REFERENCES accounts(id),expires_at TIMESTAMPTZ NOT NULL,used_at TIMESTAMPTZ);
CREATE TABLE impersonation_sessions (id TEXT PRIMARY KEY,actor_id TEXT NOT NULL REFERENCES accounts(id),effective_id TEXT NOT NULL REFERENCES accounts(id),expires_at TIMESTAMPTZ NOT NULL,revoked_at TIMESTAMPTZ);
CREATE TABLE audit_events (id BIGSERIAL PRIMARY KEY,actor_id TEXT,effective_id TEXT,action TEXT NOT NULL,resource_id TEXT,created_at TIMESTAMPTZ NOT NULL DEFAULT now());
-- Browser Auth remains public; private table access belongs to the backend.
DO $$
DECLARE table_name TEXT; client_role TEXT;
BEGIN
 FOREACH table_name IN ARRAY ARRAY['accounts','doctors','patients','devices','sessions','frame_records','device_credentials','pairing_codes','impersonation_sessions','audit_events'] LOOP
  EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY',table_name);
  FOREACH client_role IN ARRAY ARRAY['anon','authenticated'] LOOP
   IF EXISTS(SELECT 1 FROM pg_roles WHERE rolname=client_role) THEN
    EXECUTE format('REVOKE ALL PRIVILEGES ON TABLE %I FROM %I',table_name,client_role);
   END IF;
  END LOOP;
 END LOOP;
END $$;
