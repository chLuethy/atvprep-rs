//! Last-moment validation of signed manual approvals.
use super::process::PrepareMode;
use crate::{
    Error, Result,
    approval::{ApprovalItem, ApprovalVerifier, item_encode, prepare_policy_block},
    clock::format_micros,
    jobs::Jobs,
    repo::Ctx,
    schedules::{ScheduleApproval, delete_original_setting},
    snapshots::InputSnapshots,
};
use atvprep_core::encode::EncodeAllowance;
use serde_json::Value;

/// Check a changing item's approval against the identity of its opened input and report how
/// the approval lets it run. The caller must retain that same open input through execution.
///
/// # Errors
/// Rejects missing signatures, stale input, revoked/expired sessions and database failures.
pub fn authorize_prepare(
    ctx: Ctx<'_>,
    item_id: i64,
    verifier: &ApprovalVerifier,
    current_identity: &[Value; 4],
    idle_seconds: i64,
) -> Result<PrepareMode> {
    let item = Jobs(ctx)
        .get_item(item_id)?
        .ok_or_else(|| Error::Invalid("job item not found".to_owned()))?;
    let snapshot = &item.settings_snapshot;
    if let Some(reason) = prepare_policy_block(snapshot) {
        return Err(Error::Invalid(reason.to_owned()));
    }
    let recorded = InputSnapshots(ctx).get_for_item(item_id)?.ok_or_else(|| {
        Error::Invalid("The job has no recorded file identity. Queue the fix again.".to_owned())
    })?;
    let identity = [
        recorded["file_device"].clone(),
        recorded["file_inode"].clone(),
        recorded["file_ctime_ns"].clone(),
        recorded["sidecar_fingerprint"].clone(),
    ];
    if !identity[..3]
        .iter()
        .all(|v| v.as_i64().is_some_and(|n| n >= 0))
        || !identity[3].is_string()
    {
        return Err(Error::Invalid(
            "The recorded file identity is incomplete. Queue the fix again.".to_owned(),
        ));
    }
    if &identity != current_identity {
        return Err(Error::Invalid(
            "The approved file changed. Queue the fix again.".to_owned(),
        ));
    }
    if snapshot.get("schedule_id").is_some_and(|id| !id.is_null()) {
        // A job from a schedule has no session behind it: the operator's signature on the
        // schedule, valid for the schedule as it is right now, is its approval.
        return authorize_schedule_job(ctx, snapshot, verifier);
    }
    let encode = item_encode(snapshot)
        .ok_or_else(|| Error::Invalid("The job carries no encoding allowance.".to_owned()))?;
    let mode = PrepareMode {
        encode,
        delete_original: snapshot["delete_original"] == Value::Bool(true),
    };
    if let Some(reason) = verifier.check(
        snapshot,
        &ApprovalItem {
            job_id: item.job_id,
            media_id: item.media_file_id,
            action: &item.action,
            identity: &identity,
            encode: mode.encode,
            delete_original: mode.delete_original,
        },
        ctx.clock.now_micros() / 1_000_000,
    ) {
        return Err(Error::Invalid(reason.to_owned()));
    }
    let person = snapshot["approved_by"].as_i64().ok_or_else(|| {
        Error::Invalid("The approval is not bound to a signed-in session.".to_owned())
    })?;
    let now = ctx.clock.now_micros();
    let active:bool=ctx.conn.query_row("SELECT EXISTS(SELECT 1 FROM auth_sessions WHERE id=?1 AND revoked_at IS NULL AND expires_at>?2 AND last_seen_at>?3)",rusqlite::params![person,format_micros(now),format_micros(now.saturating_sub(idle_seconds.max(0).saturating_mul(1_000_000)))],|row|row.get(0))?;
    if !active {
        return Err(Error::Invalid(
            "The approving session is no longer active. Queue the fix again.".to_owned(),
        ));
    }
    Ok(mode)
}

/// A schedule's job may change files only while the schedule is on and carries a signature
/// matching its current parameters (and the current `delete_original` setting), and only if the
/// job was queued under those same parameters.
fn authorize_schedule_job(
    ctx: Ctx<'_>,
    snapshot: &Value,
    verifier: &ApprovalVerifier,
) -> Result<PrepareMode> {
    let refuse = |reason: &str| Err(Error::Invalid(reason.to_owned()));
    let Some(id) = snapshot["schedule_id"].as_i64() else {
        return refuse("The job names no valid schedule.");
    };
    let Some(row) = ScheduleApproval::load(ctx.conn, id)? else {
        return refuse("The schedule behind this job no longer exists.");
    };
    if !row.enabled {
        return refuse("The schedule is paused; its approval has ended. Turn it on again.");
    }
    let params = row.params(delete_original_setting(ctx.conn)?);
    if let Some(reason) = verifier.check_schedule(
        &params,
        row.approved_by,
        row.approved_at,
        row.approval_mac.as_deref(),
    ) {
        return refuse(reason);
    }
    if snapshot.get("schedule_params") != Some(&params.to_json()) {
        return refuse("The schedule changed after this job was queued.");
    }
    // The item's own decision is not signed; the schedule's permission is. An item may encode
    // only while the signed schedule allows it.
    let Some(encode) = item_encode(snapshot) else {
        return refuse("The job carries no encoding allowance.");
    };
    if encode != EncodeAllowance::Copy && !params.allow_reencode {
        return refuse("The schedule does not allow a re-encode.");
    }
    Ok(PrepareMode {
        encode,
        delete_original: params.delete_original,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        clock::{Clock, FixedClock},
        db::Db,
        schema,
    };
    use serde_json::json;

    #[test]
    fn authorization_requires_unchanged_input_and_live_session() {
        let db = Db::memory().unwrap();
        let mut conn = db.conn().unwrap();
        schema::migrate(&mut conn).unwrap();
        let clock = FixedClock(1_700_000_000_000_000);
        let ctx = Ctx {
            conn: conn.raw(),
            clock: &clock,
        };
        conn.raw().execute_batch("INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'Movies','/library');
            INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action) VALUES(1,1,'film.mp4','video','.mp4',1,1,'fp','present','clean','idle','none');
            INSERT INTO jobs(id,action,status,trigger) VALUES(1,'fix','queued','manual');
            INSERT INTO job_items(id,job_id,media_file_id,action,position,status) VALUES(1,1,1,'fix',0,'queued');").unwrap();
        conn.raw().execute("INSERT INTO auth_sessions(id,session_token_hash,csrf_secret_hash,expires_at,last_seen_at) VALUES(1,'token-hash','csrf-hash',?1,?2)",rusqlite::params![format_micros(clock.now_micros()+60_000_000),clock.now()]).unwrap();
        let identity = [json!(1), json!(2), json!(3), json!("sidecars")];
        InputSnapshots(ctx).record(1,&json!({"file_device":1,"file_inode":2,"file_ctime_ns":3,"sidecar_fingerprint":"sidecars"})).unwrap();
        let verifier = ApprovalVerifier::new(b"secret").unwrap();
        let signature = verifier.sign(
            &ApprovalItem {
                job_id: 1,
                media_id: 1,
                action: "fix",
                identity: &identity,
                encode: EncodeAllowance::Copy,
                delete_original: false,
            },
            1,
            clock.now_micros() / 1_000_000,
        );
        let snapshot = json!({"pipeline":"prepare","policy":"encode-v3","encode":"copy","manual_approval":true,"approved_by":1,"approved_at":clock.now_micros()/1_000_000,"approval_mac":signature});
        conn.raw()
            .execute(
                "UPDATE job_items SET settings_snapshot_json=?1 WHERE id=1",
                [snapshot.to_string()],
            )
            .unwrap();
        assert!(authorize_prepare(ctx, 1, &verifier, &identity, 3600).is_ok());
        let mut changed = identity.clone();
        changed[1] = json!(9);
        assert!(authorize_prepare(ctx, 1, &verifier, &changed, 3600).is_err());
        conn.raw()
            .execute(
                "UPDATE auth_sessions SET revoked_at=?1 WHERE id=1",
                [clock.now()],
            )
            .unwrap();
        assert!(authorize_prepare(ctx, 1, &verifier, &identity, 3600).is_err());
    }

    #[test]
    #[allow(clippy::too_many_lines)] // One walk through every way a schedule approval can lapse.
    fn a_schedule_job_needs_a_current_signature_matching_the_schedule() {
        use crate::approval::ScheduleParams;
        let db = Db::memory().unwrap();
        let mut conn = db.conn().unwrap();
        schema::migrate(&mut conn).unwrap();
        let clock = FixedClock(1_700_000_000_000_000);
        let ctx = Ctx {
            conn: conn.raw(),
            clock: &clock,
        };
        conn.raw().execute_batch("INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'Movies','/library');
            INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action) VALUES(1,1,'film.mp4','video','.mp4',1,1,'fp','present','warning','idle','none');
            INSERT INTO schedules(id,name,action,root_id,days_mask,time_of_day,max_files,enabled) VALUES(1,'s','fix',1,127,'03:00',25,1);
            INSERT INTO jobs(id,action,status,trigger) VALUES(1,'fix','queued','manual');
            INSERT INTO job_items(id,job_id,media_file_id,action,position,status) VALUES(1,1,1,'fix',0,'queued');").unwrap();
        let identity = [json!(1), json!(2), json!(3), json!("sidecars")];
        InputSnapshots(ctx).record(1,&json!({"file_device":1,"file_inode":2,"file_ctime_ns":3,"sidecar_fingerprint":"sidecars"})).unwrap();
        let verifier = ApprovalVerifier::new(b"secret").unwrap();
        let params = ScheduleParams {
            schedule_id: 1,
            action: "fix",
            root_id: Some(1),
            compliance_state: None,
            max_files: 25,
            delete_original: false,
            allow_reencode: false,
        };
        let sql = |statement: &str| conn.raw().execute(statement, []).unwrap();
        let sign = || {
            conn.raw()
                .execute(
                    "UPDATE schedules SET approved_by=1, approved_at=1700000000, approval_mac=?1 WHERE id=1",
                    [verifier.sign_schedule(&params, 1, 1_700_000_000)],
                )
                .unwrap();
        };
        let queue = |params: &ScheduleParams<'_>| {
            let snapshot = json!({"pipeline":"prepare","policy":"encode-v3","encode":"copy","manual_approval":false,
                "schedule_id":1,"schedule_params":params.to_json()});
            conn.raw()
                .execute(
                    "UPDATE job_items SET settings_snapshot_json=?1 WHERE id=1",
                    [snapshot.to_string()],
                )
                .unwrap();
        };
        let allowed = |verifier: &ApprovalVerifier| {
            authorize_prepare(ctx, 1, verifier, &identity, 3600).is_ok()
        };
        queue(&params);

        assert!(!allowed(&verifier), "never signed");
        sign();
        assert!(allowed(&verifier), "signed, and no session is involved");
        assert!(
            !allowed(&ApprovalVerifier::new(b"other").unwrap()),
            "another secret"
        );
        let mut moved = identity.clone();
        moved[1] = json!(9);
        assert!(authorize_prepare(ctx, 1, &verifier, &moved, 3600).is_err());

        sql("UPDATE schedules SET max_files=26 WHERE id=1");
        assert!(!allowed(&verifier), "max_files changed");
        sql("UPDATE schedules SET max_files=25 WHERE id=1");
        assert!(allowed(&verifier));
        sql("UPDATE schedules SET root_id=NULL WHERE id=1");
        assert!(!allowed(&verifier), "library changed");
        sql("UPDATE schedules SET root_id=1, compliance_state='clean' WHERE id=1");
        assert!(!allowed(&verifier), "filter changed");
        sql("UPDATE schedules SET compliance_state=NULL, action='convert' WHERE id=1");
        assert!(!allowed(&verifier), "action changed");
        sql("UPDATE schedules SET action='fix' WHERE id=1");
        assert!(allowed(&verifier));

        sql(
            "INSERT INTO runtime_settings(key,value_json,value_type,revision) VALUES('delete_original','true','bool',1)",
        );
        assert!(!allowed(&verifier), "delete_original is signed");
        sql("DELETE FROM runtime_settings WHERE key='delete_original'");
        assert!(allowed(&verifier));

        sql("UPDATE schedules SET allow_reencode=1 WHERE id=1");
        assert!(!allowed(&verifier), "re-encode permission changed");
        sql("UPDATE schedules SET allow_reencode=0 WHERE id=1");
        assert!(allowed(&verifier));

        sql("UPDATE schedules SET enabled=0 WHERE id=1");
        assert!(!allowed(&verifier), "paused");
        sql("UPDATE schedules SET enabled=1 WHERE id=1");

        queue(&ScheduleParams {
            max_files: 10,
            ..params
        });
        assert!(!allowed(&verifier), "queued under older parameters");
        queue(&params);
        assert!(allowed(&verifier));

        sql("DELETE FROM schedules WHERE id=1");
        assert!(!allowed(&verifier), "schedule deleted");
    }

    /// A signed manual item with one file whose approval carries `encode` and `delete_original`.
    fn manual_item(
        encode: EncodeAllowance,
        delete_original: bool,
    ) -> (crate::db::Conn, [Value; 4], i64) {
        let db = Db::memory().unwrap();
        let mut conn = db.conn().unwrap();
        schema::migrate(&mut conn).unwrap();
        let clock = FixedClock(1_700_000_000_000_000);
        let ctx = Ctx {
            conn: conn.raw(),
            clock: &clock,
        };
        conn.raw().execute_batch("INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'Movies','/library');
            INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action) VALUES(1,1,'film.mp4','video','.mp4',1,1,'fp','present','clean','idle','none');
            INSERT INTO jobs(id,action,status,trigger) VALUES(1,'fix','queued','manual');
            INSERT INTO job_items(id,job_id,media_file_id,action,position,status) VALUES(1,1,1,'fix',0,'queued');
            INSERT INTO auth_sessions(id,session_token_hash,csrf_secret_hash,expires_at,last_seen_at) VALUES(1,'token-hash','csrf-hash','2999-01-01','2999-01-01');").unwrap();
        let identity = [json!(1), json!(2), json!(3), json!("sidecars")];
        InputSnapshots(ctx).record(1,&json!({"file_device":1,"file_inode":2,"file_ctime_ns":3,"sidecar_fingerprint":"sidecars"})).unwrap();
        let signature = ApprovalVerifier::new(b"secret").unwrap().sign(
            &ApprovalItem {
                job_id: 1,
                media_id: 1,
                action: "fix",
                identity: &identity,
                encode,
                delete_original,
            },
            1,
            clock.now_micros() / 1_000_000,
        );
        let snapshot = json!({"pipeline":"prepare","policy":"encode-v3","encode":encode.as_str(),"delete_original":delete_original,"manual_approval":true,"approved_by":1,"approved_at":clock.now_micros()/1_000_000,"approval_mac":signature});
        conn.raw()
            .execute(
                "UPDATE job_items SET settings_snapshot_json=?1 WHERE id=1",
                [snapshot.to_string()],
            )
            .unwrap();
        (conn, identity, clock.now_micros())
    }

    #[test]
    fn a_manual_approval_reports_the_signed_mode() {
        let verifier = ApprovalVerifier::new(b"secret").unwrap();
        for (encode, delete_original) in [
            (EncodeAllowance::Copy, false),
            (EncodeAllowance::Audio, false),
            (EncodeAllowance::All, true),
        ] {
            let (conn, identity, now) = manual_item(encode, delete_original);
            let clock = FixedClock(now);
            let ctx = Ctx {
                conn: conn.raw(),
                clock: &clock,
            };
            assert_eq!(
                authorize_prepare(ctx, 1, &verifier, &identity, 3600).unwrap(),
                PrepareMode {
                    encode,
                    delete_original
                }
            );
        }
    }

    #[test]
    fn an_allowance_raised_on_a_manual_item_after_signing_is_refused() {
        let verifier = ApprovalVerifier::new(b"secret").unwrap();
        let (conn, identity, now) = manual_item(EncodeAllowance::Copy, false);
        let clock = FixedClock(now);
        let ctx = Ctx {
            conn: conn.raw(),
            clock: &clock,
        };
        for edit in [
            "json_set(settings_snapshot_json,'$.encode','audio')",
            "json_set(settings_snapshot_json,'$.encode','all')",
            "json_set(settings_snapshot_json,'$.delete_original',json('true'))",
            "json_set(settings_snapshot_json,'$.policy','copy-only-v2')",
            "json_remove(settings_snapshot_json,'$.encode')",
        ] {
            let (conn, _, _) = manual_item(EncodeAllowance::Copy, false);
            conn.raw()
                .execute(
                    &format!("UPDATE job_items SET settings_snapshot_json={edit}"),
                    [],
                )
                .unwrap();
            let ctx = Ctx {
                conn: conn.raw(),
                clock: &clock,
            };
            assert!(
                authorize_prepare(ctx, 1, &verifier, &identity, 3600).is_err(),
                "{edit}"
            );
        }
        assert!(authorize_prepare(ctx, 1, &verifier, &identity, 3600).is_ok());
    }

    #[test]
    fn a_schedule_item_re_encodes_only_under_a_signed_permission() {
        use crate::approval::ScheduleParams;
        let db = Db::memory().unwrap();
        let mut conn = db.conn().unwrap();
        schema::migrate(&mut conn).unwrap();
        let clock = FixedClock(1_700_000_000_000_000);
        let ctx = Ctx {
            conn: conn.raw(),
            clock: &clock,
        };
        conn.raw().execute_batch("INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'Movies','/library');
            INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action) VALUES(1,1,'film.mp4','video','.mp4',1,1,'fp','present','warning','idle','none');
            INSERT INTO schedules(id,name,action,root_id,days_mask,time_of_day,max_files,enabled) VALUES(1,'s','fix',1,127,'03:00',25,1);
            INSERT INTO jobs(id,action,status,trigger) VALUES(1,'fix','queued','manual');
            INSERT INTO job_items(id,job_id,media_file_id,action,position,status) VALUES(1,1,1,'fix',0,'queued');").unwrap();
        let identity = [json!(1), json!(2), json!(3), json!("sidecars")];
        InputSnapshots(ctx).record(1,&json!({"file_device":1,"file_inode":2,"file_ctime_ns":3,"sidecar_fingerprint":"sidecars"})).unwrap();
        let verifier = ApprovalVerifier::new(b"secret").unwrap();
        let params = |allow_reencode| ScheduleParams {
            schedule_id: 1,
            action: "fix",
            root_id: Some(1),
            compliance_state: None,
            max_files: 25,
            delete_original: false,
            allow_reencode,
        };
        let queue = |allow_reencode: bool, item_encode: EncodeAllowance| {
            let snapshot = json!({"pipeline":"prepare","policy":"encode-v3","encode":item_encode.as_str(),"manual_approval":false,
                "schedule_id":1,"schedule_params":params(allow_reencode).to_json()});
            conn.raw()
                .execute(
                    "UPDATE job_items SET settings_snapshot_json=?1 WHERE id=1",
                    [snapshot.to_string()],
                )
                .unwrap();
        };
        let sign = |allow_reencode: bool| {
            conn.raw()
                .execute(
                    "UPDATE schedules SET allow_reencode=?2, approved_by=1, approved_at=1700000000, approval_mac=?1 WHERE id=1",
                    rusqlite::params![
                        verifier.sign_schedule(&params(allow_reencode), 1, 1_700_000_000),
                        allow_reencode
                    ],
                )
                .unwrap();
        };
        let run = || authorize_prepare(ctx, 1, &verifier, &identity, 3600);
        let copying = PrepareMode {
            encode: EncodeAllowance::Copy,
            delete_original: false,
        };

        sign(false);
        queue(false, EncodeAllowance::Copy);
        assert_eq!(run().unwrap(), copying);
        // The item claims a re-encode that the signature never covered.
        queue(false, EncodeAllowance::All);
        assert!(run().is_err(), "no signed permission");

        sign(true);
        queue(true, EncodeAllowance::All);
        assert_eq!(
            run().unwrap(),
            PrepareMode {
                encode: EncodeAllowance::All,
                delete_original: false
            }
        );
        queue(true, EncodeAllowance::Copy);
        assert_eq!(run().unwrap(), copying);

        // Withdrawing the permission after queueing refuses the item, as for every parameter.
        sign(false);
        queue(true, EncodeAllowance::All);
        assert!(run().is_err(), "permission withdrawn after queueing");
    }
}
