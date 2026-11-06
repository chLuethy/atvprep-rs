-- Schedules gain the `scan` action. SQLite cannot alter a CHECK constraint, so the table is rebuilt.
-- Nothing references `schedules`, so the swap needs no foreign-key handling.
CREATE TABLE schedules_new (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    action TEXT NOT NULL CHECK (action IN ('verify','fix','convert','scan')),
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
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
INSERT INTO schedules_new(id, name, action, root_id, compliance_state, days_mask, time_of_day,
                          max_files, enabled, armed_at, last_slot_at, last_run_at, last_job_id,
                          last_result, created_at)
    SELECT id, name, action, root_id, compliance_state, days_mask, time_of_day,
           max_files, enabled, armed_at, last_slot_at, last_run_at, last_job_id,
           last_result, created_at
    FROM schedules;
DROP TABLE schedules;
ALTER TABLE schedules_new RENAME TO schedules;
