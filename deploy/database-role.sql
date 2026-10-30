-- Run as migration owner on staging first. Create the dedicated NO-BYPASSRLS
-- login/group ecg_backend separately with a password stored outside this file.
DO $$
DECLARE table_name TEXT;
BEGIN
 IF NOT EXISTS(SELECT 1 FROM pg_roles WHERE rolname='ecg_backend' AND NOT rolbypassrls AND NOT rolsuper)
 THEN RAISE EXCEPTION 'Create a non-superuser, non-BYPASSRLS ecg_backend role first'; END IF;
 FOREACH table_name IN ARRAY ARRAY['accounts','doctors','patients','devices','sessions','frame_records','device_credentials','pairing_codes','impersonation_sessions','audit_events','command_receipts','file_cleanup'] LOOP
  EXECUTE format('GRANT SELECT,INSERT,UPDATE,DELETE ON %I TO ecg_backend',table_name);
  EXECUTE format('DROP POLICY IF EXISTS backend_service_access ON %I',table_name);
  EXECUTE format('CREATE POLICY backend_service_access ON %I TO ecg_backend USING(true) WITH CHECK(true)',table_name);
 END LOOP;
END $$;
GRANT USAGE ON SCHEMA public TO ecg_backend;
GRANT SELECT ON _sqlx_migrations TO ecg_backend;
GRANT USAGE,SELECT ON audit_events_id_seq,file_cleanup_id_seq TO ecg_backend;
GRANT USAGE ON SCHEMA auth TO ecg_backend;
GRANT SELECT(id,user_id,not_after) ON auth.sessions TO ecg_backend;
-- Browser roles have no private table grants/policies. Never grant ecg_backend
-- membership to anon, authenticated, authenticator or browser-facing roles.
