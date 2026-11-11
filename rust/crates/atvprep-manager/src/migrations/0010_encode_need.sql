-- The Kodierbedarf replaces "Re-Encode nötig" (ADR 0008). What was marked becomes Video; everything
-- else is unknown until the background run has checked it against the planner.
ALTER TABLE media_files ADD COLUMN encode_need TEXT NOT NULL DEFAULT 'unknown'
    CHECK (encode_need IN ('none', 'audio', 'video', 'unknown'));
-- The Änderungsart of the same check, for the preview and the inspector.
ALTER TABLE media_files ADD COLUMN planned_change TEXT NOT NULL DEFAULT 'unknown'
    CHECK (planned_change IN ('none', 'patch', 'remux', 'audio', 'reencode', 'unknown'));
UPDATE media_files SET encode_need = 'video', planned_change = 'reencode' WHERE needs_reencode = 1;
ALTER TABLE media_files DROP COLUMN needs_reencode;
