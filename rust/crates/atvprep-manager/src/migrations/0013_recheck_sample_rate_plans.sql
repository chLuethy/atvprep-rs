-- Issue #83: Prepare now fixes the `audio-sample-rate` warning by encoding the audio track at
-- 48 kHz (AC-3/E-AC-3 to E-AC-3, AAC to AAC). Before, the planner copied such a track, so the
-- check stored no audio encode (often Kodierbedarf none and a Remux or Patch). That stored plan is
-- stale now, and nothing refreshes it on its own: the background check (`queue_need_checks`) only
-- picks up an unknown Kodierbedarf.
--
-- Affected: a file whose stored compliance reasons, from its last check, name the rule
-- `audio-sample-rate` (`[<severity>] audio-sample-rate: …`). Such a file goes back to an unknown
-- Kodierbedarf and Änderungsart, never to a guessed value: only a real check may say what Prepare
-- would do. The compliance state stays; the rule and the policy version did not change.
--
-- As in migration 0012, the finished background checks of these files stop counting as the one try
-- per file × bytes × Zielprofil revision: `need_check` becomes false, `need_check_superseded_by`
-- records why. A check still queued or running is left alone, it runs with this build.
--
-- Schedules (ADR 0006/0008): an unknown need fits only the Kodierfreigabe „Audio und Video“, which
-- takes every need anyway. Once the check stored Kodierbedarf Audio again, a schedule takes the
-- file only when its Kodierfreigabe includes audio.
UPDATE job_items
SET settings_snapshot_json = json_set(settings_snapshot_json,
                                      '$.need_check', json('false'),
                                      '$.need_check_superseded_by', 'migration 0013 (#83)')
WHERE action = 'verify'
  AND json_valid(settings_snapshot_json)
  AND json_extract(settings_snapshot_json, '$.need_check') = 1
  AND status NOT IN ('queued', 'claimed', 'running', 'cancel_requested', 'cancelling')
  AND media_file_id IN (
      SELECT m.id FROM media_files m
      WHERE json_valid(m.compliance_reasons)
        AND EXISTS (
            SELECT 1 FROM json_each(m.compliance_reasons) r
            WHERE r.value LIKE '[%] audio-sample-rate:%'
        )
  );

UPDATE media_files
SET encode_need = 'unknown',
    planned_change = 'unknown'
WHERE json_valid(compliance_reasons)
  AND EXISTS (
      SELECT 1 FROM json_each(media_files.compliance_reasons) r
      WHERE r.value LIKE '[%] audio-sample-rate:%'
  );
