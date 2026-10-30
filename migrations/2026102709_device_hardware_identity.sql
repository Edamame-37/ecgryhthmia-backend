-- Existing identities require operator review; do not infer hardware IDs from names.
ALTER TABLE devices ADD COLUMN hardware_id TEXT;
ALTER TABLE devices ADD CONSTRAINT devices_hardware_id_format CHECK (
 hardware_id IS NULL OR (
  char_length(hardware_id) BETWEEN 1 AND 128
  AND hardware_id ~ '^[A-Za-z0-9_-]+$'
 )
);
CREATE UNIQUE INDEX devices_hardware_id_unique ON devices(hardware_id)
 WHERE hardware_id IS NOT NULL;
