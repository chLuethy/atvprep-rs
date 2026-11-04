//! Verified release manifest primitives.

#[cfg(test)]
use std::fs;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use ssh_key::{PublicKey, SshSig};

use atvprep_manager::clock::Clock;
use atvprep_manager::db::Conn;
use atvprep_manager::deployment_gate::{DeploymentGate, GateStatus, Operator};
use atvprep_manager::repo::Ctx;

pub mod download;
pub mod local;
pub mod window;
pub const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
pub const MAX_ASSET_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug)]
pub enum Error {
    DrainTimeout,
    Invalid(String),
    Json(serde_json::Error),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DrainTimeout => f.write_str(
                "deployment gate did not drain before the deadline; gate remains closed",
            ),
            Self::Invalid(message) => f.write_str(message),
            Self::Json(_) => f.write_str("invalid release manifest JSON"),
        }
    }
}
impl std::error::Error for Error {}
impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}
pub type Result<T> = std::result::Result<T, Error>;

/// Gate operations accepted by the local deployment CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateCommand {
    Status,
    Close { reason: String },
    Open { generation: i64 },
    BeginMigration { generation: i64 },
    FinishMigration { generation: i64 },
}

/// Execute one local gate operation through the manager's fenced gate API.
///
/// # Errors
///
/// The gate rejects the operation or its SQLite storage cannot be read/written.
pub fn execute_gate(
    conn: &Conn,
    clock: &dyn Clock,
    operator: Option<&Operator>,
    command: &GateCommand,
) -> atvprep_manager::Result<GateStatus> {
    let gate = DeploymentGate(Ctx {
        conn: conn.raw(),
        clock,
    });
    match command {
        GateCommand::Status => gate.status(),
        GateCommand::Close { reason } => gate.close(required_operator(operator)?, reason),
        GateCommand::Open { generation } => gate.open(required_operator(operator)?, *generation),
        GateCommand::BeginMigration { generation } => {
            gate.begin_offline_migration(required_operator(operator)?, *generation)
        }
        GateCommand::FinishMigration { generation } => {
            gate.finish_offline_migration(required_operator(operator)?, *generation)
        }
    }
}

fn required_operator(operator: Option<&Operator>) -> atvprep_manager::Result<&Operator> {
    operator.ok_or_else(|| {
        atvprep_manager::Error::Invalid(
            "only an authorized operator can change the gate".to_owned(),
        )
    })
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: String,
    pub assets: Vec<Asset>,
}
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    pub platform: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

/// Verify an OpenSSH SSHSIG for a manifest against the pinned public key.
///
/// # Errors
///
/// The key/signature cannot be parsed or its `atvprep-release` signature check fails.
pub fn verify_manifest_signature(
    public_key: &str,
    manifest: &[u8],
    signature: &[u8],
) -> Result<()> {
    if public_key.len() > 4096 || manifest.len() > MAX_MANIFEST_BYTES || signature.len() > 64 * 1024
    {
        return Err(Error::Invalid(
            "release signature inputs exceed supported limits".to_owned(),
        ));
    }
    let key = PublicKey::from_openssh(public_key)
        .map_err(|error| Error::Invalid(format!("invalid release public key: {error}")))?;
    let signature = SshSig::from_pem(signature)
        .map_err(|error| Error::Invalid(format!("invalid release signature: {error}")))?;
    if key.algorithm() != ssh_key::Algorithm::Ed25519 {
        return Err(Error::Invalid("release key must use Ed25519".to_owned()));
    }
    key.verify("atvprep-release", manifest, &signature)
        .map_err(|_| Error::Invalid("release manifest signature verification failed".to_owned()))
}

/// Verify a signed manifest and then parse its validated JSON body.
///
/// Use this entry point for every installation/update path; [`parse_manifest`] alone only validates
/// untrusted JSON structure.
///
/// # Errors
///
/// The SSHSIG is invalid or the manifest fields do not satisfy release policy.
pub fn parse_signed_manifest(
    public_key: &str,
    manifest: &[u8],
    signature: &[u8],
) -> Result<Manifest> {
    verify_manifest_signature(public_key, manifest, signature)?;
    parse_manifest(manifest)
}

/// Parse and validate an unsigned release manifest before signature verification.
///
/// # Errors
///
/// The JSON or any version, platform, URL, digest or size is invalid.
pub fn parse_manifest(input: &[u8]) -> Result<Manifest> {
    if input.len() > MAX_MANIFEST_BYTES {
        return Err(Error::Invalid("release manifest is too large".to_owned()));
    }
    let manifest: Manifest = serde_json::from_slice(input)?;
    if manifest.version.len() > 128
        || semver::Version::parse(&manifest.version).is_err()
        || manifest.assets.is_empty()
        || manifest.assets.len() > 16
    {
        return Err(Error::Invalid(
            "manifest must contain a version and assets".to_owned(),
        ));
    }
    let mut platforms = std::collections::HashSet::new();
    for asset in &manifest.assets {
        if !matches!(
            asset.platform.as_str(),
            "linux-x86_64" | "linux-aarch64" | "macos-x86_64" | "macos-aarch64"
        ) || !platforms.insert(&asset.platform)
        {
            return Err(Error::Invalid(
                "manifest asset platform is invalid".to_owned(),
            ));
        }
        if !valid_asset_url(&asset.url) {
            return Err(Error::Invalid(
                "manifest asset URL must use https".to_owned(),
            ));
        }
        if asset.size == 0 || asset.size > MAX_ASSET_BYTES {
            return Err(Error::Invalid(
                "manifest asset size must be positive".to_owned(),
            ));
        }
        if asset.sha256.len() != 64 || !asset.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(Error::Invalid(
                "manifest asset sha256 is invalid".to_owned(),
            ));
        }
    }
    Ok(manifest)
}

fn valid_asset_url(value: &str) -> bool {
    reqwest::Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
    })
}

/// Refuse a downgrade according to semantic-version precedence.
///
/// # Errors
/// Either version is invalid or the release is older than the installed version.
pub fn require_upgrade(manifest: &Manifest, installed: &str) -> Result<()> {
    let release = semver::Version::parse(&manifest.version)
        .map_err(|_| Error::Invalid("invalid release version".to_owned()))?;
    let installed = semver::Version::parse(installed)
        .map_err(|_| Error::Invalid("invalid installed version".to_owned()))?;
    if release.cmp_precedence(&installed).is_lt() {
        return Err(Error::Invalid(
            "release downgrade is not allowed".to_owned(),
        ));
    }
    Ok(())
}

/// Select exactly one asset for a target platform.
///
/// # Errors
///
/// No asset or multiple assets match the platform.
pub fn select_asset<'a>(manifest: &'a Manifest, platform: &str) -> Result<&'a Asset> {
    let mut matches = manifest
        .assets
        .iter()
        .filter(|asset| asset.platform == platform);
    let Some(asset) = matches.next() else {
        return Err(Error::Invalid(
            "no release asset for this platform".to_owned(),
        ));
    };
    if matches.next().is_some() {
        return Err(Error::Invalid(
            "manifest has multiple assets for this platform".to_owned(),
        ));
    }
    Ok(asset)
}

/// Verify an asset body against its manifest hash and size.
///
/// # Errors
///
/// The size or SHA-256 digest differs from the manifest.
pub fn verify_asset(asset: &Asset, body: &[u8]) -> Result<()> {
    if u64::try_from(body.len()).unwrap_or(u64::MAX) != asset.size {
        return Err(Error::Invalid(
            "download size does not match manifest".to_owned(),
        ));
    }
    let digest = format!("{:x}", Sha256::digest(body));
    if !digest.eq_ignore_ascii_case(&asset.sha256) {
        return Err(Error::Invalid(
            "download sha256 does not match manifest".to_owned(),
        ));
    }
    Ok(())
}

/// Verify and atomically install an asset without replacing a working binary early.
///
/// # Errors
///
/// The asset is invalid, the destination has no parent, or a filesystem operation fails.
pub fn install_asset(asset: &Asset, body: &[u8], destination: &Path) -> Result<()> {
    use rustix::fs::{AtFlags, FileType, Mode, OFlags, openat, renameat, statat, unlinkat};
    verify_asset(asset, body)?;
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .canonicalize()
        .map_err(|_| Error::Invalid("cannot open installation directory".to_owned()))?;
    let name = destination
        .file_name()
        .ok_or_else(|| Error::Invalid("installation destination has no file name".to_owned()))?;
    let directory = atvprep_core::fd_fix_contract::open_library_root(&parent)
        .map_err(|_| Error::Invalid("cannot safely open installation directory".to_owned()))?;
    match statat(&directory, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile => {
            return Err(Error::Invalid(
                "installation destination must be a regular file".to_owned(),
            ));
        }
        Err(error) if error != rustix::io::Errno::NOENT => {
            return Err(Error::Invalid(
                "cannot inspect installation destination".to_owned(),
            ));
        }
        _ => {}
    }
    let mut staged = None;
    for suffix in 0..100_u8 {
        let candidate = format!(
            ".{}.atvprep-install-{}-{suffix}",
            name.to_string_lossy(),
            std::process::id()
        );
        match openat(
            &directory,
            candidate.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::from_raw_mode(0o600),
        ) {
            Ok(file) => {
                staged = Some((candidate, std::fs::File::from(file)));
                break;
            }
            Err(error) if error == rustix::io::Errno::EXIST => {}
            Err(_) => return Err(Error::Invalid("cannot create installation file".to_owned())),
        }
    }
    let Some((temporary, mut file)) = staged else {
        return Err(Error::Invalid(
            "cannot create unique installation file".to_owned(),
        ));
    };
    let result = (|| {
        file.write_all(body)
            .map_err(|_| Error::Invalid("cannot write installation file".to_owned()))?;
        file.set_permissions(std::fs::Permissions::from_mode(0o755))
            .map_err(|_| Error::Invalid("cannot set executable permissions".to_owned()))?;
        file.sync_all()
            .map_err(|_| Error::Invalid("cannot synchronize installation file".to_owned()))?;
        renameat(&directory, temporary.as_str(), &directory, name)
            .map_err(|_| Error::Invalid("cannot atomically install release".to_owned()))
    })();
    if result.is_err() {
        let _ = unlinkat(&directory, temporary.as_str(), AtFlags::empty());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    const HASH: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    fn manifest() -> Manifest {
        parse_manifest(format!(r#"{{"version":"1.0.0","assets":[{{"platform":"linux-x86_64","url":"https://example.test/a","sha256":"{HASH}","size":3}}]}}"#).as_bytes()).unwrap()
    }
    #[test]
    fn gate_cli_uses_the_manager_generation_fence() {
        let database = atvprep_manager::db::Db::memory().unwrap();
        let mut conn = database.conn().unwrap();
        atvprep_manager::schema::migrate(&mut conn).unwrap();
        let clock = atvprep_manager::clock::FixedClock(1_000_000);
        let operator = Operator::from_admin_session(1).unwrap();
        assert_eq!(
            execute_gate(&conn, &clock, None, &GateCommand::Status)
                .unwrap()
                .mode,
            "open"
        );
        let closed = execute_gate(
            &conn,
            &clock,
            Some(&operator),
            &GateCommand::Close {
                reason: "release".to_owned(),
            },
        )
        .unwrap();
        assert_eq!(closed.mode, "maintenance");
        assert!(
            execute_gate(
                &conn,
                &clock,
                Some(&operator),
                &GateCommand::Open {
                    generation: closed.generation - 1
                }
            )
            .is_err()
        );
        assert_eq!(
            execute_gate(
                &conn,
                &clock,
                Some(&operator),
                &GateCommand::Open {
                    generation: closed.generation
                }
            )
            .unwrap()
            .mode,
            "open"
        );
    }
    #[test]
    fn signed_manifest_rejects_invalid_key_or_signature() {
        assert!(parse_signed_manifest("not a key", b"{}", b"not a signature").is_err());
    }

    #[test]
    fn selects_and_verifies() {
        let data = manifest();
        let asset = select_asset(&data, "linux-x86_64").unwrap();
        verify_asset(asset, b"abc").unwrap();
        assert!(verify_asset(asset, b"bad").is_err());
    }
    #[test]
    fn installs_only_verified_bytes_atomically() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("atvprep");
        fs::write(&target, b"old").unwrap();
        let asset = manifest().assets.remove(0);
        install_asset(&asset, b"abc", &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"abc");
        assert!(install_asset(&asset, b"bad", &target).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"abc");
    }
    #[test]
    fn refuses_untrusted_manifest_fields() {
        for input in [br#"{"version":"","assets":[]}"#.as_slice(),br#"{"version":"1","assets":[{"platform":"x","url":"http://x","sha256":"x","size":0}]}"#.as_slice()]{assert!(parse_manifest(input).is_err());}
    }
}

#[cfg(test)]
mod release_policy_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn rejects_unknown_fields_duplicate_platforms_and_downgrades() {
        let asset = json!({"platform":"linux-x86_64","url":"https://example.test/release","sha256":"a".repeat(64),"size":3});
        let value = json!({"version":"1.2.3","assets":[asset.clone()]});
        let manifest = parse_manifest(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(require_upgrade(&manifest, "1.2.2").is_ok());
        assert!(require_upgrade(&manifest, "1.2.3+build.1").is_ok());
        assert!(require_upgrade(&manifest, "2.0.0").is_err());
        let mut invalid = value.clone();
        invalid["secret"] = json!("must not appear");
        let error = parse_manifest(&serde_json::to_vec(&invalid).unwrap())
            .unwrap_err()
            .to_string();
        assert!(!error.contains("secret"));
        let mut duplicate = value.clone();
        duplicate["assets"] = json!([asset.clone(), asset]);
        assert!(parse_manifest(&serde_json::to_vec(&duplicate).unwrap()).is_err());
        for version in ["1", "v1.2.3", "", "not-a-version"] {
            let mut invalid = value.clone();
            invalid["version"] = json!(version);
            assert!(parse_manifest(&serde_json::to_vec(&invalid).unwrap()).is_err());
        }
        for url in [
            "http://example.test/release",
            "https://user:password@example.test/a",
            "https://example.test/a#fragment",
        ] {
            let mut invalid = value.clone();
            invalid["assets"][0]["url"] = json!(url);
            assert!(parse_manifest(&serde_json::to_vec(&invalid).unwrap()).is_err());
        }
    }

    #[test]
    fn authenticates_real_ed25519_signatures_and_rejects_changed_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let key = directory.path().join("key");
        let status = std::process::Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-C", "release-test", "-f"])
            .arg(&key)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(status.status.success());
        let body=br#"{"version":"1.0.0","assets":[{"platform":"linux-x86_64","url":"https://example.test/a","sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","size":3}]}"#;
        let path = directory.path().join("manifest.json");
        std::fs::write(&path, body).unwrap();
        let status = std::process::Command::new("ssh-keygen")
            .args(["-Y", "sign", "-f"])
            .arg(&key)
            .args(["-n", "atvprep-release"])
            .arg(&path)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(status.status.success());
        let public = std::fs::read_to_string(key.with_extension("pub")).unwrap();
        let signature = std::fs::read(directory.path().join("manifest.json.sig")).unwrap();
        assert!(parse_signed_manifest(&public, body, &signature).is_ok());
        let mut changed = body.to_vec();
        changed.push(b' ');
        assert!(parse_signed_manifest(&public, &changed, &signature).is_err());
        let other = directory.path().join("other-key");
        assert!(
            std::process::Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                .arg(&other)
                .output()
                .unwrap()
                .status
                .success()
        );
        let wrong_public = std::fs::read_to_string(other.with_extension("pub")).unwrap();
        assert!(parse_signed_manifest(&wrong_public, body, &signature).is_err());
        // A valid signature in a different protocol namespace is not a release.
        std::fs::remove_file(directory.path().join("manifest.json.sig")).unwrap();
        assert!(
            std::process::Command::new("ssh-keygen")
                .args(["-Y", "sign", "-f"])
                .arg(&key)
                .args(["-n", "unrelated-protocol"])
                .arg(&path)
                .output()
                .unwrap()
                .status
                .success()
        );
        let wrong_namespace = std::fs::read(directory.path().join("manifest.json.sig")).unwrap();
        assert!(parse_signed_manifest(&public, body, &wrong_namespace).is_err());
    }
}
