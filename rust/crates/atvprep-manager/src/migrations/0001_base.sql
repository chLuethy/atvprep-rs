-- The manager's base schema (a fresh start; the Python Alembic history is not replayed).

CREATE TABLE library_roots (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    canonical_path TEXT NOT NULL UNIQUE,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE scan_runs (
    id INTEGER PRIMARY KEY,
    root_id INTEGER NOT NULL REFERENCES library_roots(id),
    generation INTEGER NOT NULL,
    trigger TEXT NOT NULL DEFAULT 'scheduled' CHECK (trigger IN ('scheduled','manual')),
    status TEXT NOT NULL DEFAULT 'queued'
        CHECK (status IN ('queued','claimed','running','succeeded','failed','cancelled','interrupted')),
    claimed_by TEXT,
    lease_expires_at TEXT,
    heartbeat_at TEXT,
    attempt INTEGER NOT NULL DEFAULT 0,
    status_version INTEGER NOT NULL DEFAULT 0,
    fencing_token INTEGER NOT NULL DEFAULT 0,
    seen_count INTEGER NOT NULL DEFAULT 0,
    discovered_count INTEGER NOT NULL DEFAULT 0,
    changed_count INTEGER NOT NULL DEFAULT 0,
    missing_count INTEGER NOT NULL DEFAULT 0,
    probe_failed_count INTEGER NOT NULL DEFAULT 0,
    started_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    finished_at TEXT,
    error TEXT,
    CONSTRAINT uq_scan_root_generation UNIQUE (root_id, generation)
);
CREATE INDEX ix_scan_runs_root_id ON scan_runs(root_id);

CREATE TABLE scheduler_leases (
    id INTEGER PRIMARY KEY,
    owner_id TEXT,
    lease_expires_at TEXT,
    heartbeat_at TEXT,
    interval_seconds INTEGER NOT NULL DEFAULT 3600,
    fencing_token INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE maintenance_leases (
    id INTEGER PRIMARY KEY,
    owner_id TEXT,
    lease_expires_at TEXT,
    heartbeat_at TEXT,
    fencing_token INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE media_executor_leases (
    id INTEGER PRIMARY KEY,
    owner_id TEXT,
    lease_expires_at TEXT,
    heartbeat_at TEXT,
    fencing_token INTEGER NOT NULL DEFAULT 0
);
INSERT INTO media_executor_leases (id, fencing_token) VALUES (1, 0);

CREATE TABLE deployment_gate (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    mode TEXT NOT NULL CHECK (mode IN ('open','maintenance')),
    phase TEXT NOT NULL CHECK (phase IN ('steady','offline-migration')),
    generation INTEGER NOT NULL CHECK (generation >= 0),
    reason TEXT,
    changed_by_kind TEXT,
    changed_by_ref TEXT,
    changed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
INSERT INTO deployment_gate (id, mode, phase, generation) VALUES (1, 'open', 'steady', 0);

CREATE TABLE deployment_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    generation INTEGER NOT NULL,
    kind TEXT NOT NULL,
    actor_kind TEXT,
    at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
INSERT INTO deployment_events (generation, kind, actor_kind) VALUES (0, 'fresh-open', 'migration');

CREATE TABLE deployment_permits (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('job','scan','schedule')),
    holder TEXT NOT NULL,
    token TEXT NOT NULL,
    pid INTEGER,
    host TEXT,
    proc_start TEXT,
    generation INTEGER NOT NULL,
    issued_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    released_at TEXT,
    abandoned_by TEXT,
    abandon_evidence TEXT
);
CREATE INDEX idx_deployment_permits_active ON deployment_permits(released_at);

CREATE TABLE media_files (
    id INTEGER PRIMARY KEY,
    root_id INTEGER NOT NULL REFERENCES library_roots(id),
    relative_path TEXT NOT NULL,
    kind TEXT NOT NULL,
    suffix TEXT NOT NULL,
    size INTEGER NOT NULL,
    mtime_ns INTEGER NOT NULL,
    fingerprint TEXT NOT NULL,
    scanned_device INTEGER,
    scanned_inode INTEGER,
    scanned_ctime_ns INTEGER,
    sidecar_fingerprint TEXT NOT NULL DEFAULT '',
    discovery_state TEXT NOT NULL CHECK (discovery_state IN ('present','unsettled','missing')),
    compliance_state TEXT NOT NULL
        CHECK (compliance_state IN ('unknown','clean','warning','blocker','probe_failed')),
    compliance_reasons TEXT NOT NULL DEFAULT '[]',
    workflow_state TEXT NOT NULL
        CHECK (workflow_state IN ('idle','action_required','queued','running','failed')),
    desired_action TEXT NOT NULL CHECK (desired_action IN ('none','verify','fix','convert')),
    last_seen_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_success_at TEXT,
    last_error TEXT,
    status_version INTEGER NOT NULL DEFAULT 0,
    last_seen_scan_id INTEGER REFERENCES scan_runs(id),
    last_seen_generation INTEGER,
    conversion_output_media_id INTEGER REFERENCES media_files(id),
    last_converted_input_fingerprint TEXT,
    dismissed_at TEXT,
    CONSTRAINT uq_media_root_relative_path UNIQUE (root_id, relative_path)
);
CREATE INDEX ix_media_files_root_id ON media_files(root_id);
CREATE INDEX ix_media_files_last_seen_scan_id ON media_files(last_seen_scan_id);
CREATE INDEX idx_media_workflow ON media_files(workflow_state, desired_action);

CREATE TABLE schedules (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    action TEXT NOT NULL CHECK (action IN ('verify','fix','convert')),
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

CREATE TABLE sidecars (
    id INTEGER PRIMARY KEY,
    media_file_id INTEGER NOT NULL REFERENCES media_files(id),
    relative_path TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    size INTEGER NOT NULL DEFAULT 0,
    mtime_ns INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uq_sidecar_media_path UNIQUE (media_file_id, relative_path)
);
CREATE INDEX ix_sidecars_media_file_id ON sidecars(media_file_id);

CREATE TABLE inspections (
    id INTEGER PRIMARY KEY,
    media_file_id INTEGER NOT NULL REFERENCES media_files(id),
    source_fingerprint TEXT NOT NULL,
    input_fingerprint TEXT NOT NULL,
    tool_name TEXT NOT NULL,
    tool_version TEXT NOT NULL,
    result TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX ix_inspections_media_file_id ON inspections(media_file_id);
CREATE INDEX idx_inspections_media_created ON inspections(media_file_id, created_at);

CREATE TABLE jobs (
    id INTEGER PRIMARY KEY,
    action TEXT NOT NULL CHECK (action IN ('inspect','verify','fix','convert')),
    status TEXT NOT NULL DEFAULT 'queued'
        CHECK (status IN ('queued','running','cancel_requested','succeeded','partial_success','failed','cancelled')),
    trigger TEXT NOT NULL DEFAULT 'automatic' CHECK (trigger IN ('automatic','manual','retry')),
    origin_job_id INTEGER REFERENCES jobs(id),
    settings_revision INTEGER NOT NULL DEFAULT 0,
    status_version INTEGER NOT NULL DEFAULT 0,
    settings_snapshot_json TEXT NOT NULL DEFAULT '{}',
    requested_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    started_at TEXT,
    finished_at TEXT,
    cancel_requested_at TEXT,
    cleared_at TEXT,
    error TEXT,
    total_count INTEGER NOT NULL DEFAULT 0,
    succeeded_count INTEGER NOT NULL DEFAULT 0,
    failed_count INTEGER NOT NULL DEFAULT 0,
    cancelled_count INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_jobs_status_requested ON jobs(status, requested_at);

CREATE TABLE job_items (
    id INTEGER PRIMARY KEY,
    job_id INTEGER NOT NULL REFERENCES jobs(id) ON DELETE RESTRICT,
    media_file_id INTEGER NOT NULL REFERENCES media_files(id) ON DELETE RESTRICT,
    action TEXT NOT NULL CHECK (action IN ('inspect','verify','fix','convert')),
    position INTEGER NOT NULL,
    origin_item_id INTEGER REFERENCES job_items(id) ON DELETE RESTRICT,
    status TEXT NOT NULL DEFAULT 'queued'
        CHECK (status IN ('queued','claimed','running','cancel_requested','cancelling',
                          'succeeded','failed','cancelled','interrupted','stale_input')),
    status_version INTEGER NOT NULL DEFAULT 0,
    attempt INTEGER NOT NULL DEFAULT 0,
    claimed_by TEXT,
    lease_expires_at TEXT,
    heartbeat_at TEXT,
    executor_fencing_token INTEGER,
    input_snapshot_json TEXT NOT NULL DEFAULT '{}',
    settings_snapshot_json TEXT NOT NULL DEFAULT '{}',
    started_at TEXT,
    finished_at TEXT,
    exit_code INTEGER,
    error_code TEXT,
    error_detail TEXT,
    output_media_file_id INTEGER REFERENCES media_files(id) ON DELETE RESTRICT,
    log_path TEXT,
    log_tail TEXT,
    progress INTEGER
);
CREATE INDEX ix_job_items_job_id ON job_items(job_id);
CREATE INDEX ix_job_items_media_file_id ON job_items(media_file_id);
CREATE INDEX idx_job_items_claim ON job_items(status, lease_expires_at, id);
CREATE UNIQUE INDEX uq_active_job_item_media ON job_items(media_file_id)
    WHERE action IN ('fix','convert')
      AND status NOT IN ('succeeded','failed','cancelled','interrupted','stale_input');

CREATE TABLE input_snapshots (
    id INTEGER PRIMARY KEY,
    job_item_id INTEGER NOT NULL REFERENCES job_items(id) ON DELETE CASCADE,
    snapshot_version INTEGER NOT NULL DEFAULT 1,
    snapshot_json TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uq_input_snapshot_item UNIQUE (job_item_id)
);

CREATE TABLE events (
    id INTEGER PRIMARY KEY,
    job_id INTEGER REFERENCES jobs(id) ON DELETE RESTRICT,
    item_id INTEGER REFERENCES job_items(id) ON DELETE RESTRICT,
    topic TEXT NOT NULL,
    entity_type TEXT NOT NULL,
    entity_id INTEGER NOT NULL,
    payload_json TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX ix_events_job_id ON events(job_id);
CREATE INDEX ix_events_item_id ON events(item_id);
CREATE INDEX idx_events_expiry ON events(expires_at, id);
CREATE INDEX idx_events_entity ON events(entity_type, entity_id, id);
CREATE TRIGGER prevent_event_update BEFORE UPDATE ON events
BEGIN SELECT RAISE(ABORT, 'events are append-only'); END;
CREATE TRIGGER prevent_event_delete BEFORE DELETE ON events
WHEN event_delete_authorized(OLD.id) IS NOT 1
BEGIN SELECT RAISE(ABORT, 'events are append-only'); END;

CREATE TABLE runtime_settings (
    key TEXT PRIMARY KEY,
    value_json TEXT NOT NULL,
    value_type TEXT NOT NULL,
    revision INTEGER NOT NULL,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE settings_revision (
    id INTEGER PRIMARY KEY,
    revision INTEGER NOT NULL DEFAULT 0
);
INSERT INTO settings_revision (id, revision) VALUES (1, 0);

CREATE TABLE settings_audit (
    id INTEGER PRIMARY KEY,
    key TEXT NOT NULL REFERENCES runtime_settings(key) ON DELETE RESTRICT,
    revision INTEGER NOT NULL,
    actor TEXT NOT NULL,
    old_value_json TEXT,
    new_value_json TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX ix_settings_audit_key ON settings_audit(key);
-- One bulk update writes several audit rows under one revision, so this index is not unique.
CREATE INDEX ix_settings_audit_revision ON settings_audit(revision);

CREATE TABLE auth_sessions (
    id INTEGER PRIMARY KEY,
    session_token_hash TEXT NOT NULL UNIQUE,
    csrf_secret_hash TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    revoked_at TEXT
);
CREATE INDEX idx_auth_sessions_expiry ON auth_sessions(expires_at, revoked_at);

CREATE TABLE csrf_tokens (
    id INTEGER PRIMARY KEY,
    auth_session_id INTEGER NOT NULL REFERENCES auth_sessions(id) ON DELETE CASCADE,
    token_hash TEXT NOT NULL UNIQUE,
    method TEXT NOT NULL,
    normalized_path TEXT NOT NULL,
    action TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at TEXT NOT NULL,
    used_at TEXT
);
CREATE INDEX ix_csrf_tokens_auth_session_id ON csrf_tokens(auth_session_id);
CREATE INDEX idx_csrf_tokens_session_expiry ON csrf_tokens(auth_session_id, expires_at);

CREATE TABLE idempotency_records (
    id INTEGER PRIMARY KEY,
    auth_session_id INTEGER REFERENCES auth_sessions(id) ON DELETE CASCADE,
    method TEXT NOT NULL,
    normalized_route TEXT NOT NULL,
    key TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    response_status INTEGER NOT NULL,
    response_json TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (auth_session_id, method, normalized_route, key)
);
CREATE INDEX ix_idempotency_records_auth_session_id ON idempotency_records(auth_session_id);
