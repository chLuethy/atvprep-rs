//! Durable browser sessions backed by the manager database.

use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use std::io::Read;
use std::path::Path;

use atvprep_manager::clock::format_micros;
use atvprep_manager::repo::Ctx;
use rusqlite::OptionalExtension;

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

use crate::random;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionError;
impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid session")
    }
}
impl std::error::Error for SessionError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedSession {
    pub id: i64,
    pub token: String,
    pub csrf_token: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CsrfError;
impl std::fmt::Display for CsrfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid CSRF token")
    }
}
impl std::error::Error for CsrfError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionCookiePolicy {
    pub name: &'static str,
    pub path: &'static str,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: &'static str,
}

impl SessionCookiePolicy {
    #[must_use]
    pub const fn new(secure: bool) -> Self {
        Self {
            name: if secure {
                "__Host-atvprep_session"
            } else {
                "atvprep_session"
            },
            path: "/",
            secure,
            http_only: true,
            same_site: "Strict",
        }
    }
}

pub struct AuthService<'a> {
    ctx: Ctx<'a>,
    absolute_ttl: i64,
    idle_ttl: i64,
    csrf_ttl: i64,
}

#[allow(clippy::missing_errors_doc)]
impl<'a> AuthService<'a> {
    pub fn new(ctx: Ctx<'a>, absolute_ttl: i64, idle_ttl: i64) -> Result<Self, SessionError> {
        Self::with_csrf_ttl(ctx, absolute_ttl, idle_ttl, 900)
    }

    pub fn with_csrf_ttl(
        ctx: Ctx<'a>,
        absolute_ttl: i64,
        idle_ttl: i64,
        csrf_ttl: i64,
    ) -> Result<Self, SessionError> {
        if !(1..=86_400).contains(&absolute_ttl) || !(1..=28_800).contains(&idle_ttl) {
            return Err(SessionError);
        }
        if !(1..=3_600).contains(&csrf_ttl) {
            return Err(SessionError);
        }
        Ok(Self {
            ctx,
            absolute_ttl,
            idle_ttl,
            csrf_ttl,
        })
    }
    pub fn create(&self) -> Result<IssuedSession, SessionError> {
        self.atomic(|| self.create_inner())
    }

    fn create_inner(&self) -> Result<IssuedSession, SessionError> {
        let token = random::token(32).map_err(|_| SessionError)?;
        let csrf_token = random::token(32).map_err(|_| SessionError)?;
        let csrf_secret = random::token(32).map_err(|_| SessionError)?;
        let now = self.ctx.clock.now_micros();
        let expires_at = format_micros(now + self.absolute_ttl * 1_000_000);
        self.ctx.conn.execute("INSERT INTO auth_sessions(session_token_hash,csrf_secret_hash,created_at,last_seen_at,expires_at) VALUES(?1,?2,?3,?3,?4)", rusqlite::params![hash(&token),hash(&csrf_secret),format_micros(now),expires_at]).map_err(|_| SessionError)?;
        let id = self.ctx.conn.last_insert_rowid();
        self.issue_csrf_for_id(
            id,
            &csrf_token,
            "DELETE",
            "/api/v1/auth/session",
            "auth-session",
            now,
            expires_at.clone(),
        )?;
        Ok(IssuedSession {
            id,
            token,
            csrf_token,
            expires_at,
        })
    }
    pub fn authenticate(&self, token: &str) -> Result<i64, SessionError> {
        let supplied = hash(token);
        let now_micros = self.ctx.clock.now_micros();
        let now = format_micros(now_micros);
        let row: Option<(i64,String,String,String,Option<String>)>=self.ctx.conn.query_row("SELECT id,session_token_hash,expires_at,last_seen_at,revoked_at FROM auth_sessions WHERE session_token_hash=?1",[&supplied],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional().map_err(|_|SessionError)?;
        let verified = matches_hash(&supplied, row.as_ref().map(|row| row.1.as_str()));
        let Some((id, _, expires, last_seen, revoked)) = row else {
            return Err(SessionError);
        };
        if !verified
            || revoked.is_some()
            || now >= expires
            || last_seen <= format_micros(now_micros.saturating_sub(self.idle_ttl * 1_000_000))
        {
            return Err(SessionError);
        }
        let refresh = (self.idle_ttl * 500_000).min(300_000_000);
        if last_seen <= format_micros(now_micros.saturating_sub(refresh)) {
            self.ctx
                .conn
                .execute(
                    "UPDATE auth_sessions SET last_seen_at=?1 WHERE id=?2",
                    rusqlite::params![now, id],
                )
                .map_err(|_| SessionError)?;
        }
        Ok(id)
    }

    pub fn rotate(&self, token: &str) -> Result<IssuedSession, SessionError> {
        self.atomic(|| self.rotate_inner(token))
    }

    fn rotate_inner(&self, token: &str) -> Result<IssuedSession, SessionError> {
        let id = self.authenticate(token)?;
        let new_token = random::token(32).map_err(|_| SessionError)?;
        let csrf_token = random::token(32).map_err(|_| SessionError)?;
        let csrf_secret = random::token(32).map_err(|_| SessionError)?;
        let now = self.ctx.clock.now_micros();
        let changed = self.ctx.conn.execute(
            "UPDATE auth_sessions SET session_token_hash=?1, csrf_secret_hash=?2, last_seen_at=?3 WHERE id=?4 AND revoked_at IS NULL AND session_token_hash=?5",
            rusqlite::params![hash(&new_token), hash(&csrf_secret), format_micros(now), id, hash(token)],
        ).map_err(|_| SessionError)?;
        if changed != 1 {
            return Err(SessionError);
        }
        let expires_at: String = self
            .ctx
            .conn
            .query_row(
                "SELECT expires_at FROM auth_sessions WHERE id=?1",
                [id],
                |row| row.get(0),
            )
            .map_err(|_| SessionError)?;
        self.issue_csrf_for_id(
            id,
            &csrf_token,
            "DELETE",
            "/api/v1/auth/session",
            "auth-session",
            now,
            expires_at.clone(),
        )?;
        Ok(IssuedSession {
            id,
            token: new_token,
            csrf_token,
            expires_at,
        })
    }

    pub fn revoke(&self, token: &str) -> Result<(), SessionError> {
        self.atomic(|| self.revoke_inner(token))
    }

    fn revoke_inner(&self, token: &str) -> Result<(), SessionError> {
        let id = self.authenticate(token)?;
        let now = format_micros(self.ctx.clock.now_micros());
        self.ctx
            .conn
            .execute(
                "UPDATE auth_sessions SET revoked_at=?1 WHERE id=?2 AND revoked_at IS NULL",
                rusqlite::params![now, id],
            )
            .map_err(|_| SessionError)?;
        self.ctx
            .conn
            .execute(
                "UPDATE csrf_tokens SET used_at=?1 WHERE auth_session_id=?2 AND used_at IS NULL",
                rusqlite::params![now, id],
            )
            .map_err(|_| SessionError)?;
        Ok(())
    }

    fn atomic<T>(&self, work: impl FnOnce() -> Result<T, SessionError>) -> Result<T, SessionError> {
        self.ctx
            .conn
            .execute_batch("SAVEPOINT atvprep_auth")
            .map_err(|_| SessionError)?;
        match work() {
            Ok(result) => {
                self.ctx
                    .conn
                    .execute_batch("RELEASE atvprep_auth")
                    .map_err(|_| SessionError)?;
                Ok(result)
            }
            Err(error) => {
                self.ctx
                    .conn
                    .execute_batch("ROLLBACK TO atvprep_auth; RELEASE atvprep_auth")
                    .map_err(|_| SessionError)?;
                Err(error)
            }
        }
    }

    pub fn issue_csrf(
        &self,
        token: &str,
        method: &str,
        path: &str,
        action: &str,
    ) -> Result<String, SessionError> {
        let id = self.authenticate(token)?;
        let csrf = random::token(32).map_err(|_| SessionError)?;
        let now = self.ctx.clock.now_micros();
        let expires: String = self
            .ctx
            .conn
            .query_row(
                "SELECT expires_at FROM auth_sessions WHERE id=?1",
                [id],
                |row| row.get(0),
            )
            .map_err(|_| SessionError)?;
        self.issue_csrf_for_id(id, &csrf, method, path, action, now, expires)?;
        Ok(csrf)
    }

    #[allow(clippy::too_many_arguments)]
    fn issue_csrf_for_id(
        &self,
        id: i64,
        raw: &str,
        method: &str,
        path: &str,
        action: &str,
        now: i64,
        session_expires: String,
    ) -> Result<(), SessionError> {
        let expires = std::cmp::min(
            session_expires,
            format_micros(now + self.csrf_ttl * 1_000_000),
        );
        self.ctx.conn.execute("INSERT INTO csrf_tokens(auth_session_id,token_hash,method,normalized_path,action,created_at,expires_at) VALUES(?1,?2,?3,?4,?5,?6,?7)", rusqlite::params![id, hash(raw), method.to_ascii_uppercase(), path, action, format_micros(now), expires]).map_err(|_| SessionError)?;
        Ok(())
    }

    pub fn validate_csrf(
        &self,
        token: &str,
        csrf: &str,
        method: &str,
        path: &str,
        action: &str,
        allow_used: bool,
    ) -> Result<bool, CsrfError> {
        let id = self.authenticate(token).map_err(|_| CsrfError)?;
        let supplied = hash(csrf);
        let now = format_micros(self.ctx.clock.now_micros());
        let row: Option<(i64, String, Option<String>, String, String, String)> = self.ctx.conn.query_row("SELECT id,token_hash,used_at,method,normalized_path,action FROM csrf_tokens WHERE token_hash=?1 AND auth_session_id=?2 AND expires_at>?3", rusqlite::params![supplied, id, now], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional().map_err(|_| CsrfError)?;
        let verified = matches_hash(&supplied, row.as_ref().map(|row| row.1.as_str()));
        let Some((csrf_id, _, used, stored_method, stored_path, stored_action)) = row else {
            return Err(CsrfError);
        };
        if !verified
            || stored_method != method.to_ascii_uppercase()
            || stored_path != path
            || stored_action != action
        {
            return Err(CsrfError);
        }
        if used.is_some() {
            return if allow_used { Ok(true) } else { Err(CsrfError) };
        }
        let changed = self.ctx.conn.execute("UPDATE csrf_tokens SET used_at=?1 WHERE id=?2 AND auth_session_id=?3 AND used_at IS NULL AND expires_at>?1 AND method=?4 AND normalized_path=?5 AND action=?6", rusqlite::params![now, csrf_id, id, method.to_ascii_uppercase(), path, action]).map_err(|_| CsrfError)?;
        if changed == 1 {
            Ok(true)
        } else {
            Err(CsrfError)
        }
    }
}
fn matches_hash(supplied: &str, stored: Option<&str>) -> bool {
    const ABSENT: &str = "0000000000000000000000000000000000000000000000000000000000000000";
    stored
        .unwrap_or(ABSENT)
        .as_bytes()
        .ct_eq(supplied.as_bytes())
        .into()
}

fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

/// Load a private, regular master-token file without following symlinks.
///
/// # Errors
/// Returns an error when the file is missing, unsafe, unreadable, or too short.
pub fn load_master_token(path: &Path) -> Result<String, std::io::Error> {
    let fd = rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::NONBLOCK
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )?;
    let mut file = std::fs::File::from(fd);
    let metadata = file.metadata()?;
    #[cfg(unix)]
    let unsafe_permissions = metadata.permissions().mode() & 0o077 != 0;
    #[cfg(not(unix))]
    let unsafe_permissions = false;
    #[cfg(unix)]
    let wrong_owner = metadata.uid() != rustix::process::getuid().as_raw();
    #[cfg(not(unix))]
    let wrong_owner = false;
    if !metadata.file_type().is_file() || unsafe_permissions || wrong_owner {
        return Err(std::io::Error::other("master credential is invalid"));
    }
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    let token = contents.trim().to_owned();
    if token.chars().count() < 43 {
        return Err(std::io::Error::other("master credential is invalid"));
    }
    Ok(token)
}

#[must_use]
pub fn verify_master_token(candidate: &str, expected: &str) -> bool {
    candidate.as_bytes().ct_eq(expected.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use atvprep_manager::clock::FixedClock;
    use atvprep_manager::db::Db;
    use atvprep_manager::schema;

    const NOW: i64 = 1_700_000_000_000_000;

    fn service(clock: &FixedClock) -> (Db, AuthService<'_>) {
        let db = Db::memory().unwrap();
        let mut migration = db.conn().unwrap();
        schema::migrate(&mut migration).unwrap();
        drop(migration);
        let conn = Box::leak(Box::new(db.conn().unwrap()));
        let service = AuthService::with_csrf_ttl(
            Ctx {
                conn: conn.raw(),
                clock,
            },
            60,
            20,
            10,
        )
        .unwrap();
        (db, service)
    }

    #[test]
    fn sessions_expire_rotate_and_revoke() {
        let clock = FixedClock(NOW);
        let (_db, service) = service(&clock);
        let issued = service.create().unwrap();
        assert_eq!(service.authenticate(&issued.token).unwrap(), issued.id);
        assert_ne!(issued.token, issued.csrf_token);
        let rotated = service.rotate(&issued.token).unwrap();
        assert!(service.authenticate(&issued.token).is_err());
        assert_eq!(service.authenticate(&rotated.token).unwrap(), issued.id);
        service.revoke(&rotated.token).unwrap();
        assert!(service.authenticate(&rotated.token).is_err());
    }

    #[test]
    fn csrf_is_bound_to_request_and_single_use() {
        let clock = FixedClock(NOW);
        let (_db, service) = service(&clock);
        let issued = service.create().unwrap();
        let csrf = service
            .issue_csrf(&issued.token, "POST", "/api/v1/jobs", "create")
            .unwrap();
        assert!(
            service
                .validate_csrf(
                    &issued.token,
                    &csrf,
                    "POST",
                    "/api/v1/jobs",
                    "create",
                    false
                )
                .unwrap()
        );
        assert!(
            service
                .validate_csrf(&issued.token, &csrf, "POST", "/api/v1/jobs", "create", true)
                .unwrap()
        );
        assert!(
            service
                .validate_csrf(
                    &issued.token,
                    &csrf,
                    "POST",
                    "/api/v1/jobs",
                    "create",
                    false
                )
                .is_err()
        );
    }

    #[test]
    fn csrf_and_session_expire_at_their_boundaries() {
        let clock = FixedClock(NOW);
        let (_db, service) = service(&clock);
        let issued = service.create().unwrap();
        let csrf = service
            .issue_csrf(&issued.token, "POST", "/api/v1/jobs", "create")
            .unwrap();
        assert!(
            service
                .validate_csrf(
                    &issued.token,
                    &csrf,
                    "DELETE",
                    "/api/v1/jobs",
                    "create",
                    false
                )
                .is_err()
        );
        let later = FixedClock(NOW + 10_000_000);
        let expired_csrf = AuthService::with_csrf_ttl(
            Ctx {
                conn: service.ctx.conn,
                clock: &later,
            },
            60,
            20,
            10,
        )
        .unwrap();
        assert!(
            expired_csrf
                .validate_csrf(
                    &issued.token,
                    &csrf,
                    "POST",
                    "/api/v1/jobs",
                    "create",
                    false
                )
                .is_err()
        );
        let idle = FixedClock(NOW + 30_000_000);
        let expired_session = AuthService::with_csrf_ttl(
            Ctx {
                conn: service.ctx.conn,
                clock: &idle,
            },
            60,
            20,
            10,
        )
        .unwrap();
        assert!(expired_session.authenticate(&issued.token).is_err());
        assert!(
            AuthService::with_csrf_ttl(
                Ctx {
                    conn: service.ctx.conn,
                    clock: &FixedClock(NOW)
                },
                0,
                1,
                1
            )
            .is_err()
        );
    }

    #[test]
    fn activity_does_not_extend_absolute_expiry() {
        let clock = FixedClock(NOW);
        let (_db, service) = service(&clock);
        let issued = service.create().unwrap();
        for seconds in [10, 20, 30, 40, 50, 59] {
            let current = FixedClock(NOW + seconds * 1_000_000);
            let active = AuthService::with_csrf_ttl(
                Ctx {
                    conn: service.ctx.conn,
                    clock: &current,
                },
                60,
                20,
                10,
            )
            .unwrap();
            assert_eq!(active.authenticate(&issued.token).unwrap(), issued.id);
        }
        let deadline = FixedClock(NOW + 60_000_000);
        let expired = AuthService::with_csrf_ttl(
            Ctx {
                conn: service.ctx.conn,
                clock: &deadline,
            },
            60,
            20,
            10,
        )
        .unwrap();
        assert!(expired.authenticate(&issued.token).is_err());
    }

    #[test]
    fn master_token_requires_a_private_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("master");
        std::fs::write(&path, "a".repeat(43)).unwrap();
        #[cfg(unix)]
        {
            std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600))
                .unwrap();
        }
        assert_eq!(load_master_token(&path).unwrap().len(), 43);
        std::fs::write(&path, "é".repeat(22)).unwrap();
        assert!(load_master_token(&path).is_err());
        std::fs::write(&path, "é".repeat(43)).unwrap();
        assert_eq!(load_master_token(&path).unwrap(), "é".repeat(43));
        std::fs::write(&path, "x".repeat(43)).unwrap();

        assert!(verify_master_token("same", "same"));
        assert!(!verify_master_token("same", "other"));
        #[cfg(unix)]
        {
            let link = dir.path().join("link");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            assert!(load_master_token(&link).is_err());
            std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o644))
                .unwrap();
            assert!(load_master_token(&path).is_err());
        }
    }

    #[test]
    fn failed_bootstrap_csrf_rolls_back_session_creation() {
        let clock = FixedClock(NOW);
        let (_db, service) = service(&clock);
        service.ctx.conn.execute_batch("CREATE TRIGGER reject_csrf BEFORE INSERT ON csrf_tokens BEGIN SELECT RAISE(ABORT, 'refused'); END;").unwrap();
        assert!(service.create().is_err());
        let count: i64 = service
            .ctx
            .conn
            .query_row("SELECT count(*) FROM auth_sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn concurrent_csrf_claim_has_one_winner() {
        use std::sync::{Arc, Barrier};
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("auth.db")).unwrap();
        let mut conn = db.conn().unwrap();
        schema::migrate(&mut conn).unwrap();
        let clock = FixedClock(NOW);
        let service = AuthService::with_csrf_ttl(
            Ctx {
                conn: conn.raw(),
                clock: &clock,
            },
            60,
            20,
            10,
        )
        .unwrap();
        let session = service.create().unwrap();
        let csrf = service
            .issue_csrf(&session.token, "POST", "/jobs", "create")
            .unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let workers: Vec<_> = (0..2)
            .map(|_| {
                let db = db.clone();
                let barrier = Arc::clone(&barrier);
                let token = session.token.clone();
                let csrf = csrf.clone();
                std::thread::spawn(move || {
                    let conn = db.conn().unwrap();
                    let clock = FixedClock(NOW);
                    let service = AuthService::with_csrf_ttl(
                        Ctx {
                            conn: conn.raw(),
                            clock: &clock,
                        },
                        60,
                        20,
                        10,
                    )
                    .unwrap();
                    barrier.wait();
                    service
                        .validate_csrf(&token, &csrf, "POST", "/jobs", "create", false)
                        .is_ok()
                })
            })
            .collect();
        assert_eq!(
            workers
                .into_iter()
                .map(|worker| worker.join().unwrap())
                .filter(|won| *won)
                .count(),
            1
        );
    }
}
