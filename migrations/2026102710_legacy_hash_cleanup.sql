-- Apply only after verified backup and retirement of the legacy password-login binary.
-- A successful old ledger entry does not guarantee that this column stayed removed.
ALTER TABLE accounts DROP COLUMN IF EXISTS password_hash;
