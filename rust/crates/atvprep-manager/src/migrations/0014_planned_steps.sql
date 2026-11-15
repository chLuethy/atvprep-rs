-- The steps the planner named at the last check (#98), as a JSON array of objects `{kind, ...}`.
-- Before, they were stored as `[plan] ...` text among the compliance reasons. Such old reasons stay
-- as they are and are read as text steps; the next check stores the steps here.
ALTER TABLE media_files ADD COLUMN planned_steps TEXT NOT NULL DEFAULT '[]';
