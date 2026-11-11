-- Repairing and converting become the one action „Vorbereiten“ ('prepare'), and the yes/no re-encode
-- permission becomes the schedule's Kodierfreigabe (ADR 0008): on becomes 'all', off becomes 'copy'.
-- Both are part of the operator's signature, so every schedule that changes files loses its approval;
-- a switched-on one is paused, with the reason in its history. Turning it on again signs anew.
--
-- SQLite cannot alter a CHECK constraint, so `schedules` is rebuilt. Its history references it with
-- ON DELETE CASCADE, and dropping the old table would empty that history, so `schedule_runs` is set
-- aside first and rebuilt afterwards with the same columns and indexes.
CREATE TABLE schedule_runs_keep AS SELECT * FROM schedule_runs;
DROP TABLE schedule_runs;

CREATE TABLE schedules_new (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    action TEXT NOT NULL CHECK (action IN ('verify','prepare','scan')),
    root_id INTEGER REFERENCES library_roots(id),
    compliance_state TEXT,
    days_mask INTEGER NOT NULL CHECK (days_mask BETWEEN 1 AND 127),
    time_of_day TEXT NOT NULL,
    max_files INTEGER NOT NULL DEFAULT 25 CHECK (max_files BETWEEN 1 AND 500),
    enabled INTEGER NOT NULL DEFAULT 0,
    armed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_slot_at TEXT,
    last_run_at TEXT,
    last_job_id INTEGER,
    last_result TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    cron TEXT,
    approved_by INTEGER,
    approved_at INTEGER,
    approval_mac TEXT,
    encode TEXT NOT NULL DEFAULT 'copy' CHECK (encode IN ('copy','audio','all'))
);
INSERT INTO schedules_new(id, name, action, root_id, compliance_state, days_mask, time_of_day,
                          max_files, enabled, armed_at, last_slot_at, last_run_at, last_job_id,
                          last_result, created_at, cron, approved_by, approved_at, approval_mac, encode)
    SELECT id, name, CASE WHEN action IN ('fix','convert') THEN 'prepare' ELSE action END,
           root_id, compliance_state, days_mask, time_of_day, max_files, enabled, armed_at,
           last_slot_at, last_run_at, last_job_id, last_result, created_at, cron,
           approved_by, approved_at, approval_mac,
           CASE WHEN action IN ('fix','convert') AND allow_reencode = 1 THEN 'all' ELSE 'copy' END
    FROM schedules;
DROP TABLE schedules;
ALTER TABLE schedules_new RENAME TO schedules;

CREATE TABLE schedule_runs (
    id INTEGER PRIMARY KEY,
    schedule_id INTEGER NOT NULL REFERENCES schedules(id) ON DELETE CASCADE,
    slot TEXT,
    started_at TEXT NOT NULL,
    trigger TEXT NOT NULL CHECK (trigger IN ('scheduled','manual')),
    result TEXT NOT NULL,
    job_id INTEGER,
    scan_id INTEGER,
    kind TEXT NOT NULL DEFAULT 'run' CHECK (kind IN ('run','missed'))
);
INSERT INTO schedule_runs(id, schedule_id, slot, started_at, trigger, result, job_id, scan_id, kind)
    SELECT id, schedule_id, slot, started_at, trigger, result, job_id, scan_id, kind
    FROM schedule_runs_keep;
DROP TABLE schedule_runs_keep;
CREATE INDEX schedule_runs_by_schedule ON schedule_runs(schedule_id, id DESC);
CREATE UNIQUE INDEX schedule_runs_missed_slot ON schedule_runs(schedule_id, slot) WHERE kind = 'missed';

INSERT INTO schedule_runs(schedule_id, slot, started_at, trigger, result)
SELECT id, NULL, strftime('%Y-%m-%d %H:%M:%f', 'now') || '000', 'scheduled',
       'Paused: the approval now covers the encoding allowance and the target profile. Turn the schedule on again to approve it.'
FROM schedules WHERE enabled = 1 AND action = 'prepare';

UPDATE schedules
SET last_result = 'Paused: the approval now covers the encoding allowance and the target profile. Turn the schedule on again to approve it.',
    last_run_at = strftime('%Y-%m-%d %H:%M:%f', 'now') || '000'
WHERE enabled = 1 AND action = 'prepare';

UPDATE schedules SET enabled = 0, approved_by = NULL, approved_at = NULL, approval_mac = NULL
WHERE action = 'prepare';
