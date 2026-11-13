-- Issue #62: an MP4 keeps an embedded SDH track only through its hearing_impaired disposition, and
-- the old check read only the title. It saw the SDH sidecar as still to merge and stored a planned
-- change (usually Remux) for a file that is ready. Nothing refreshes that on its own: a Prepare that
-- changes nothing starts no check, and the background check (`queue_need_checks`) only picks up an
-- unknown Kodierbedarf.
--
-- Affected: a file with a stored planned change other than none and a sidecar whose name marks it
-- SDH, read like `sub_info_from_srt_name` ("sdh", ".hi.", "hearing"; on the whole relative path,
-- which may take in a few more files). Such a file goes back to an unknown Kodierbedarf and
-- Änderungsart, never to none: only a real check may call it ready. The compliance state stays, it
-- comes from verify's violations, which the SDH detection does not feed.
--
-- The background check tries each file × bytes × Zielprofil revision once. These files were
-- checked exactly so after migration 0010, so that earlier try would block the new one and leave
-- them „unbekannt“. Their finished background checks therefore stop counting as that try:
-- `need_check` becomes false, `need_check_superseded_by` records why. A check still queued or
-- running is left alone, it runs with this build and blocks nothing.
--
-- Schedules (ADR 0008): an unknown need fits only the Kodierfreigabe „Audio und Video“, which
-- takes every need anyway. A schedule therefore gains no file through this; one with „Nur kopieren“
-- or „Audio“ skips the file until the check has stored its need again.
UPDATE job_items
SET settings_snapshot_json = json_set(settings_snapshot_json,
                                      '$.need_check', json('false'),
                                      '$.need_check_superseded_by', 'migration 0012 (#62)')
WHERE action = 'verify'
  AND json_valid(settings_snapshot_json)
  AND json_extract(settings_snapshot_json, '$.need_check') = 1
  AND status NOT IN ('queued', 'claimed', 'running', 'cancel_requested', 'cancelling')
  AND media_file_id IN (
      SELECT m.id FROM media_files m
      WHERE m.planned_change <> 'none'
        AND EXISTS (
            SELECT 1 FROM sidecars s
            WHERE s.media_file_id = m.id
              AND (lower(s.relative_path) LIKE '%sdh%'
                   OR lower(s.relative_path) LIKE '%.hi.%'
                   OR lower(s.relative_path) LIKE '%hearing%')
        )
  );

UPDATE media_files
SET encode_need = 'unknown',
    planned_change = 'unknown'
WHERE planned_change <> 'none'
  AND EXISTS (
      SELECT 1 FROM sidecars s
      WHERE s.media_file_id = media_files.id
        AND (lower(s.relative_path) LIKE '%sdh%'
             OR lower(s.relative_path) LIKE '%.hi.%'
             OR lower(s.relative_path) LIKE '%hearing%')
  );
