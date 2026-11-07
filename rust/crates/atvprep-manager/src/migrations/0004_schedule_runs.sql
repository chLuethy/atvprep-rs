-- History of schedule runs: the scheduled slots and the manual "Run now" calls, newest 10 per schedule.
-- `job_id` and `scan_id` carry no foreign key: a pruned job or scan must not block or erase history.
CREATE TABLE schedule_runs (
    id INTEGER PRIMARY KEY,
    schedule_id INTEGER NOT NULL REFERENCES schedules(id) ON DELETE CASCADE,
    slot TEXT,
    started_at TEXT NOT NULL,
    trigger TEXT NOT NULL CHECK (trigger IN ('scheduled','manual')),
    result TEXT NOT NULL,
    job_id INTEGER,
    scan_id INTEGER
);
CREATE INDEX schedule_runs_by_schedule ON schedule_runs(schedule_id, id DESC);
