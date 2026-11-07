-- A schedule that changes files says explicitly whether it may take files that need a re-encode
-- (ADR 0007). The flag is part of the operator's signature, so existing schedules start with it off
-- and every switched-on schedule that changes files loses its approval and is paused, with the reason
-- in its history. Turning it on again signs the new parameters.
ALTER TABLE schedules ADD COLUMN allow_reencode INTEGER NOT NULL DEFAULT 0 CHECK (allow_reencode IN (0, 1));

INSERT INTO schedule_runs(schedule_id, slot, started_at, trigger, result)
SELECT id, NULL, strftime('%Y-%m-%d %H:%M:%f', 'now') || '000', 'scheduled',
       'Paused: the approval now covers whether re-encoding is allowed (off). Turn the schedule on again to approve it.'
FROM schedules WHERE enabled = 1 AND action IN ('fix', 'convert');

UPDATE schedules
SET last_result = 'Paused: the approval now covers whether re-encoding is allowed (off). Turn the schedule on again to approve it.',
    last_run_at = strftime('%Y-%m-%d %H:%M:%f', 'now') || '000'
WHERE enabled = 1 AND action IN ('fix', 'convert');

UPDATE schedules SET enabled = 0, approved_by = NULL, approved_at = NULL, approval_mac = NULL
WHERE action IN ('fix', 'convert');
