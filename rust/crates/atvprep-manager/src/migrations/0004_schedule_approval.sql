-- A schedule that changes files carries the operator's signature (HMAC over its id and parameters),
-- set when it is turned on and cleared when those parameters change or it is paused.
ALTER TABLE schedules ADD COLUMN approved_by INTEGER;
ALTER TABLE schedules ADD COLUMN approved_at INTEGER;
ALTER TABLE schedules ADD COLUMN approval_mac TEXT;
