ALTER TABLE devices ADD COLUMN telemetry JSONB;
-- Fail migration on conflicting assignments; resolve explicitly in staging first.
CREATE UNIQUE INDEX patients_device_unique ON patients(device_id) WHERE device_id IS NOT NULL;
DO $$
DECLARE table_name TEXT;
BEGIN
 FOREACH table_name IN ARRAY ARRAY['accounts','doctors','patients','devices','sessions','frame_records','device_credentials','pairing_codes','impersonation_sessions','audit_events'] LOOP
  EXECUTE format('REVOKE ALL PRIVILEGES ON TABLE %I FROM PUBLIC',table_name);
 END LOOP;
END $$;
-- Legacy password hashes have no authentication role. Login uses Supabase Auth.
ALTER TABLE accounts DROP COLUMN IF EXISTS password_hash;
