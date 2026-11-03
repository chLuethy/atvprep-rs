//! Last-moment validation of signed manual approvals.
use crate::{
    Error, Result,
    approval::{ApprovalItem, ApprovalVerifier, prepare_policy_block},
    clock::format_micros,
    jobs::Jobs,
    repo::Ctx,
    snapshots::InputSnapshots,
};
use serde_json::Value;

/// Check a changing item's approval against the identity of its opened input.
/// The caller must retain that same open input through execution.
///
/// # Errors
/// Rejects missing signatures, stale input, revoked/expired sessions and database failures.
pub fn authorize_prepare(
    ctx: Ctx<'_>,
    item_id: i64,
    verifier: &ApprovalVerifier,
    current_identity: &[Value; 4],
    idle_seconds: i64,
) -> Result<()> {
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
    if let Some(reason) = verifier.check(
        snapshot,
        &ApprovalItem {
            job_id: item.job_id,
            media_id: item.media_file_id,
            action: &item.action,
            identity: &identity,
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
    Ok(())
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
            },
            1,
            clock.now_micros() / 1_000_000,
        );
        let snapshot = json!({"pipeline":"prepare","policy":"copy-only-v1","manual_approval":true,"approved_by":1,"approved_at":clock.now_micros()/1_000_000,"approval_mac":signature});
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
}
