-- A schedule's history also notes slots the manager did not trigger in time ('missed'). Such a slot
-- gets at most one entry; the partial unique index is the guarantee, however many ticks see it.
-- Older entries are runs. Additive only: nothing is rebuilt.
ALTER TABLE schedule_runs ADD COLUMN kind TEXT NOT NULL DEFAULT 'run' CHECK (kind IN ('run','missed'));
CREATE UNIQUE INDEX schedule_runs_missed_slot ON schedule_runs(schedule_id, slot) WHERE kind = 'missed';
