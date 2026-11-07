-- "Re-Encode nötig" becomes a stored property of the check result instead of being guessed from the
-- reason texts. Existing rows get it once, derived from the old reasons; from now on the compliance
-- projection writes it together with the state. Additive only: nothing is rebuilt.
ALTER TABLE media_files ADD COLUMN needs_reencode INTEGER NOT NULL DEFAULT 0 CHECK (needs_reencode IN (0, 1));
UPDATE media_files SET needs_reencode = 1 WHERE compliance_reasons LIKE '%] video-reencode:%';
