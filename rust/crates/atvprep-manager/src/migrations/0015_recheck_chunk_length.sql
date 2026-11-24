-- Issue #144 (ADR 0010): the check now judges the chunk length of a file. An MP4 whose audio or
-- video chunks average under a quarter second (what ffmpeg writes: a sample or two per chunk) is a
-- warning of the new rule `chunk-length` and a Remux, never a Patch. Every stored result predates
-- that: a file stored as clean or as a Patch may be exactly such a file, and nothing refreshes it on
-- its own, because the background check (`queue_need_checks`) only picks up an unknown Kodierbedarf.
--
-- Affected: every file whose stored Änderungsart is none or patch. Only for such a file the chunk
-- length can change the plan (to a Remux); a file stored as remux, audio or reencode stays what it
-- is. The affected file goes back to an unknown Kodierbedarf and Änderungsart, never to a guessed
-- value: only a real check may say what Prepare would do. The compliance state stays until that
-- check has run; the new rule decides it anew.
--
-- As in migrations 0012 and 0013, the finished background checks of these files stop counting as the
-- one try per file × bytes × Zielprofil revision: `need_check` becomes false,
-- `need_check_superseded_by` records why. A check still queued or running is left alone, it runs
-- with this build.
--
-- Schedules (ADR 0006/0008/0010): an unknown need fits only the Kodierfreigabe „Audio und Video“.
-- Once the check has stored the need again, a file whose only reason is the chunk length is no
-- candidate of a schedule. The signature and the parameters of a schedule do not change.
UPDATE job_items
SET settings_snapshot_json = json_set(settings_snapshot_json,
                                      '$.need_check', json('false'),
                                      '$.need_check_superseded_by', 'migration 0015 (#144)')
WHERE action = 'verify'
  AND json_valid(settings_snapshot_json)
  AND json_extract(settings_snapshot_json, '$.need_check') = 1
  AND status NOT IN ('queued', 'claimed', 'running', 'cancel_requested', 'cancelling')
  AND media_file_id IN (
      SELECT m.id FROM media_files m
      WHERE m.planned_change IN ('none', 'patch')
  );

UPDATE media_files
SET encode_need = 'unknown',
    planned_change = 'unknown'
WHERE planned_change IN ('none', 'patch');
