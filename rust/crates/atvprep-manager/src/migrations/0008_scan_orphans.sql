-- A finished scan notes the leftover partial rewrites it found: count, bytes and a short list.
-- NULL means none were found (or the scan predates the report). Additive only.
ALTER TABLE scan_runs ADD COLUMN orphans_json TEXT;
