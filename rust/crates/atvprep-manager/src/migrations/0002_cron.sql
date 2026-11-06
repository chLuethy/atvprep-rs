-- Schedules run on a cron expression. days_mask and time_of_day stay as legacy columns (still
-- filled in, and the fallback when cron is NULL), but this does NOT make the schema downward
-- compatible: a binary older than this release refuses the database because user_version is
-- newer than it knows. Rolling back needs the pre-release database backup (see DEPLOYMENT.md).
-- A NULL cron means "derive it from the legacy columns".
ALTER TABLE schedules ADD COLUMN cron TEXT;

-- Monday = bit 0 becomes cron weekday 1 ... Saturday = bit 5 becomes 6, Sunday = bit 6 becomes 0.
UPDATE schedules SET cron =
    CAST(CAST(substr(time_of_day, 4, 2) AS INTEGER) AS TEXT) || ' ' ||
    CAST(CAST(substr(time_of_day, 1, 2) AS INTEGER) AS TEXT) || ' * * ' ||
    substr(
        (CASE WHEN days_mask & 64 THEN ',0' ELSE '' END) ||
        (CASE WHEN days_mask & 1 THEN ',1' ELSE '' END) ||
        (CASE WHEN days_mask & 2 THEN ',2' ELSE '' END) ||
        (CASE WHEN days_mask & 4 THEN ',3' ELSE '' END) ||
        (CASE WHEN days_mask & 8 THEN ',4' ELSE '' END) ||
        (CASE WHEN days_mask & 16 THEN ',5' ELSE '' END) ||
        (CASE WHEN days_mask & 32 THEN ',6' ELSE '' END),
        2);
