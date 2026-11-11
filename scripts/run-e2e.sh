#!/usr/bin/env bash
# Run the Playwright smoke test against the Rust server serving the built SPA.
# Needs sqlite3, ffprobe, the frontend dependencies and a Chromium (`npx playwright install chromium`).
# The journeys expect a fixed fixture (Sample Movie, Failed Movie, a queued and a failed job); it is
# inserted into a freshly migrated temporary database before the server serves the tests.
set -euo pipefail
cd "$(dirname "$0")/.."

(cd frontend && npm run build)
# Build from rust/ so rustup picks up rust/rust-toolchain.toml (CI has no default toolchain).
(cd rust && cargo build --locked -p atvprep)

work=$(mktemp -d)
server=
cleanup() {
  [ -n "$server" ] && kill "$server" 2>/dev/null || true
  rm -rf "$work"
}
trap cleanup EXIT

mkdir "$work/media"
chmod 700 "$work"
umask 077
token=$(head -c 48 /dev/urandom | base64 | tr -d '=+/\n')
printf '%s' "$token" > "$work/master-token"
cat > "$work/config.toml" <<TOML
[server]
bind_host = "127.0.0.1"
bind_port = 8765
allowed_origins = ["http://127.0.0.1:8765"]
[storage]
database_path = "$work/manager.sqlite"
[library]
roots = ["$work/media"]
[auth]
master_token_file = "$work/master-token"
approval_secret = "e2e-approval-secret-$(head -c 12 /dev/urandom | base64 | tr -d '=+/\n')"
session_cookie_secure = false
[atvprep]
ffprobe = "$(command -v ffprobe)"
TOML

start() {
  rust/target/debug/atvprep manager api --config "$work/config.toml" --web-dist frontend/dist &
  server=$!
  for _ in $(seq 100); do
    curl -fsS http://127.0.0.1:8765/api/health/ready >/dev/null 2>&1 && return 0
    kill -0 "$server" 2>/dev/null || break
    sleep 0.2
  done
  echo "manager did not become ready" >&2
  return 1
}
stop() {
  kill "$server" 2>/dev/null || true
  wait "$server" 2>/dev/null || true
  server=
}

# First start applies the migrations, then the fixture goes in while the server is down.
start
stop
printf 'synthetic e2e media' > "$work/media/Sample Movie.mp4"
size=$(wc -c < "$work/media/Sample Movie.mp4" | tr -d ' ')
fingerprint=$(shasum -a 256 "$work/media/Sample Movie.mp4" | cut -d' ' -f1)
# A leftover partial rewrite beside the source, as a crash would leave it, and the scan that found it.
leftover=".tmp_fixmeta_enc.Sample Movie.4711-0a1b2c3d.mp4"
printf 'partial rewrite' > "$work/media/$leftover"
sqlite3 "$work/manager.sqlite" <<SQL
INSERT INTO scan_runs(id,root_id,generation,status,finished_at,orphans_json) VALUES(100,1,100,'succeeded',datetime('now'),'{"count":1,"bytes":15,"files":[{"relative_path":"$leftover","size":15,"modified_at":"2026-11-01 10:00:00"}]}');
INSERT OR IGNORE INTO library_roots(id,name,canonical_path) VALUES(1,'E2E Library','$(cd "$work/media" && pwd -P)');
INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,scanned_device,scanned_inode,scanned_ctime_ns,discovery_state,compliance_state,workflow_state,desired_action,encode_need,planned_change) VALUES
 (1,1,'Sample Movie.mp4','movie','.mp4',$size,1,'$fingerprint',1,1,1,'present','clean','idle','none','none','none'),
 (2,1,'Failed Movie.mp4','movie','.mp4',$size,1,'$fingerprint',1,2,1,'present','warning','action_required','fix','none','remux'),
 (3,1,'Interrupted Movie.mp4','movie','.mp4',$size,1,'$fingerprint',1,3,1,'present','warning','action_required','fix','none','remux'),
 (4,1,'Old Codec Movie.mp4','movie','.mp4',$size,1,'$fingerprint',1,4,1,'present','warning','action_required','fix','video','reencode');
INSERT INTO inspections(media_file_id,source_fingerprint,input_fingerprint,tool_name,tool_version,result) VALUES(1,'$fingerprint','$fingerprint','ffprobe','e2e','{"format":{"format_name":"mp4"}}');
INSERT INTO jobs(id,action,status,trigger,total_count,failed_count,error) VALUES(1,'verify','queued','manual',1,0,NULL),(2,'fix','failed','manual',1,1,'synthetic e2e failure'),(3,'fix','failed','manual',1,1,'worker stopped');
INSERT INTO job_items(id,job_id,media_file_id,action,position,status,attempt,error_code,error_detail,input_snapshot_json,log_tail,finished_at) VALUES
 (1,1,1,'verify',1,'queued',0,NULL,NULL,'{"relative_path":"Sample Movie.mp4"}','synthetic executor log',NULL),
 (2,2,2,'fix',1,'failed',1,'synthetic_failure','synthetic e2e failure','{"relative_path":"Failed Movie.mp4"}','synthetic executor log',NULL),
 (3,3,3,'fix',1,'interrupted',1,'worker_stopped','The worker stopped while this ran.','{"relative_path":"Interrupted Movie.mp4"}','synthetic executor log','2999-01-01T00:00:00Z');
INSERT INTO events(job_id,topic,entity_type,entity_id,payload_json,expires_at) VALUES(1,'job.created','job',1,'{"action":"verify","source":"e2e"}',datetime('now','+1 day'));
-- Keep the fixture stable: the real workers must not pick it up.
INSERT OR IGNORE INTO scheduler_leases(id) VALUES(1);
INSERT OR IGNORE INTO media_executor_leases(id) VALUES(1);
UPDATE scheduler_leases SET owner_id='e2e-fixture',lease_expires_at='2999-01-01 00:00:00.000000' WHERE id=1;
UPDATE media_executor_leases SET owner_id='e2e-fixture',lease_expires_at='2999-01-01 00:00:00.000000' WHERE id=1;
SQL
start

cd frontend
E2E_MASTER_TOKEN="$token" npx playwright test --project=chromium "$@"
