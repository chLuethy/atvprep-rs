-- Issue #62: an MP4 keeps an embedded SDH track only through its hearing_impaired disposition, and
-- the old detection read only the title. A file whose stored warning says nothing but "sidecar
-- SRT(s) not yet merged" may therefore be fine; the schedule kept preparing it, and a Prepare that
-- changes nothing never asks for a new check. Such a file goes back to „unbekannt“, never to clean:
-- only a real check may call it ready. The unknown Kodierbedarf is what the background check
-- (`queue_need_checks`) picks up, so it is checked again without the operator.
--
-- Only rows in the state warning whose every reason is this one: a warning with further reasons
-- stays a warning whatever the sidecars say, still needs a Prepare, and that Prepare checks the
-- rewritten file anew. A blocker is not about this reason either. Neither is touched.
UPDATE media_files
SET compliance_state = 'unknown',
    compliance_reasons = '["Stored warning about unmerged sidecar SRTs may be outdated; a new check is required."]',
    encode_need = 'unknown',
    planned_change = 'unknown'
WHERE compliance_state = 'warning'
  AND json_valid(compliance_reasons)
  AND json_array_length(compliance_reasons) > 0
  AND NOT EXISTS (
      SELECT 1 FROM json_each(media_files.compliance_reasons) AS reason
      WHERE reason.value NOT LIKE '%sidecar SRT(s) not yet merged'
  );
