//! Signed local release roundtrip through the actual deployment CLI.
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

struct Bundle {
    manifest: PathBuf,
    signature: PathBuf,
    asset: PathBuf,
}
fn bundle(
    directory: &Path,
    label: &str,
    version: &str,
    binary_version: &str,
    key: &Path,
) -> Bundle {
    let source = directory.join(format!("{label}.rs"));
    let asset = directory.join(format!("{label}-binary"));
    std::fs::write(
        &source,
        format!(r#"fn main() {{
            let args: Vec<String> = std::env::args().collect();
            if args.get(1).map(String::as_str) == Some("manager") {{
                let generation = args.last().unwrap();
                println!("{{{{\"migrated\":true,\"schema_version\":1,\"generation\":{{}}}}}}", generation);
            }} else {{ println!("atvprep {binary_version}"); }}
        }}"#),
    )
    .unwrap();
    let build = Command::new("rustc")
        .arg("--edition=2024")
        .arg("--crate-name")
        .arg("release_fixture")
        .arg(&source)
        .arg("-o")
        .arg(&asset)
        .output()
        .unwrap();
    assert!(build.status.success());
    let body = std::fs::read(&asset).unwrap();
    let platform = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    let document = json!({"version":version,"assets":[{"platform":platform,"url":"https://example.test/release","sha256":format!("{:x}",Sha256::digest(&body)),"size":body.len()}]});
    let manifest = directory.join(format!("{label}.json"));
    std::fs::write(&manifest, serde_json::to_vec(&document).unwrap()).unwrap();
    let signed = Command::new("ssh-keygen")
        .args(["-Y", "sign", "-f"])
        .arg(key)
        .args(["-n", "atvprep-release"])
        .arg(&manifest)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(signed.status.success());
    Bundle {
        manifest,
        signature: directory.join(format!("{label}.json.sig")),
        asset,
    }
}
fn deploy(action: &str, bundle: &Bundle, key: &Path, destination: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_atvprep"))
        .args(["deploy", action, "--manifest"])
        .arg(&bundle.manifest)
        .arg("--signature")
        .arg(&bundle.signature)
        .arg("--trusted-key")
        .arg(key)
        .arg("--destination")
        .arg(destination)
        .arg("--asset-file")
        .arg(&bundle.asset)
        .output()
        .unwrap()
}
fn version(path: &Path) -> String {
    let output = Command::new(path).arg("--version").output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn assert_unsafe_parent_preserves_binary(bundle: &Bundle, key: &Path, original: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let parent = original.parent().unwrap().join("unsafe-install");
    std::fs::create_dir(&parent).unwrap();
    let destination = parent.join("atvprep");
    std::fs::copy(original, &destination).unwrap();
    let before = std::fs::read(&destination).unwrap();
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o777)).unwrap();
    let refused = deploy("update", bundle, key, &destination);
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(!refused.status.success());
    assert_eq!(std::fs::read(&destination).unwrap(), before);
    assert!(!parent.join(".atvprep-install.lock").exists());
}

#[test]
fn signed_install_update_rollback_and_failed_validation_preserve_working_binary() {
    let directory = tempfile::tempdir().unwrap();
    let key = directory.path().join("trusted");
    let generated = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "release-test", "-f"])
        .arg(&key)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(generated.status.success());
    let first = bundle(directory.path(), "first", "1.0.0", "1.0.0", &key);
    let second = bundle(directory.path(), "second", "2.0.0", "2.0.0", &key);
    let broken = bundle(directory.path(), "broken", "3.0.0", "0.0.0", &key);
    let public = key.with_extension("pub");
    let destination = directory.path().join("atvprep");
    let installed = deploy("install", &first, &public, &destination);
    assert!(
        installed.status.success(),
        "{}",
        String::from_utf8_lossy(&installed.stderr)
    );
    assert_eq!(version(&destination), "atvprep 1.0.0");
    assert_unsafe_parent_preserves_binary(&second, &public, &destination);
    let updated = deploy("update", &second, &public, &destination);
    assert!(
        updated.status.success(),
        "{}",
        String::from_utf8_lossy(&updated.stderr)
    );
    assert_eq!(version(&destination), "atvprep 2.0.0");
    let downgrade = deploy("update", &first, &public, &destination);
    assert!(!downgrade.status.success());
    assert_eq!(version(&destination), "atvprep 2.0.0");
    assert!(
        deploy("rollback", &first, &public, &destination)
            .status
            .success()
    );
    assert_eq!(version(&destination), "atvprep 1.0.0");
    let before = std::fs::read(&destination).unwrap();
    assert!(
        !deploy("update", &broken, &public, &destination)
            .status
            .success()
    );
    assert_eq!(std::fs::read(&destination).unwrap(), before);
    let database = directory.path().join("manager.sqlite");
    let db = atvprep_manager::db::Db::open(&database).unwrap();
    atvprep_manager::schema::migrate(&mut db.conn().unwrap()).unwrap();
    let backup = directory.path().join("backup");
    std::fs::create_dir(&backup).unwrap();
    let window = |execute: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_atvprep"));
        command
            .args(["deploy", "release-window", "--manifest"])
            .arg(&second.manifest)
            .arg("--signature")
            .arg(&second.signature)
            .arg("--asset-file")
            .arg(&second.asset)
            .arg("--trusted-key")
            .arg(&public)
            .arg("--destination")
            .arg(&destination)
            .arg("--database")
            .arg(&database)
            .arg("--backup-dir")
            .arg(&backup);
        if execute {
            command.args(["--execute", "--services-stopped", "--imports-stopped"]);
        }
        command.output().unwrap()
    };
    let dry = window(false);
    assert!(
        dry.status.success(),
        "{}",
        String::from_utf8_lossy(&dry.stderr)
    );
    assert!(!backup.join("window.jsonl").exists());
    assert_eq!(std::fs::read(&destination).unwrap(), before);
    let executed = window(true);
    assert!(
        executed.status.success(),
        "{}",
        String::from_utf8_lossy(&executed.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&executed.stdout).unwrap();
    assert_eq!(result["gate_closed"], true);
    assert_eq!(result["backup_created"], true);
    assert_eq!(version(&destination), "atvprep 2.0.0");
    let before = std::fs::read(&destination).unwrap();
    std::fs::write(&first.asset, b"tampered asset").unwrap();
    let downgrade = deploy("update", &first, &public, &destination);
    assert!(!downgrade.status.success());
    assert_eq!(std::fs::read(&destination).unwrap(), before);
}

#[test]
fn servarr_hook_install_is_signed_idempotent_and_refuses_downgrades() {
    use std::os::unix::fs::MetadataExt;
    let directory = tempfile::tempdir().unwrap();
    let key = directory.path().join("trusted");
    assert!(
        Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&key)
            .output()
            .unwrap()
            .status
            .success()
    );
    let first = bundle(directory.path(), "hook-first", "1.0.0", "1.0.0", &key);
    let second = bundle(directory.path(), "hook-second", "2.0.0", "2.0.0", &key);
    let inconsistent = bundle(
        directory.path(),
        "hook-inconsistent",
        "2.0.0",
        "0.0.0",
        &key,
    );
    let destination = directory.path().join("atvprep-hook");
    let invoke = |bundle: &Bundle, application: &str| {
        Command::new(env!("CARGO_BIN_EXE_atvprep"))
            .args([
                "deploy",
                "servarr",
                "--application",
                application,
                "--manifest",
            ])
            .arg(&bundle.manifest)
            .arg("--signature")
            .arg(&bundle.signature)
            .arg("--trusted-key")
            .arg(key.with_extension("pub"))
            .arg("--asset-file")
            .arg(&bundle.asset)
            .arg("--destination")
            .arg(&destination)
            .output()
            .unwrap()
    };
    let installed = invoke(&first, "sonarr");
    assert!(
        installed.status.success(),
        "{}",
        String::from_utf8_lossy(&installed.stderr)
    );
    let inode = std::fs::metadata(&destination).unwrap().ino();
    assert!(invoke(&first, "sonarr").status.success());
    assert_eq!(std::fs::metadata(&destination).unwrap().ino(), inode);
    assert!(invoke(&second, "radarr").status.success());
    let bytes = std::fs::read(&destination).unwrap();
    assert!(!invoke(&first, "whisparr").status.success());
    assert!(!invoke(&inconsistent, "sonarr").status.success());
    assert!(!invoke(&second, "sonarr; echo injection").status.success());
    assert_eq!(std::fs::read(&destination).unwrap(), bytes);
}

#[test]
fn every_multicall_name_starts_the_actual_binary() {
    let directory = tempfile::tempdir().unwrap();
    for name in ["atvprep", "atvprep-hook", "atvprep-manager"] {
        let path = directory.path().join(name);
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_atvprep"), &path).unwrap();
        let version = Command::new(&path).arg("--version").output().unwrap();
        assert!(version.status.success());
        assert!(
            String::from_utf8(version.stdout)
                .unwrap()
                .trim()
                .ends_with(env!("CARGO_PKG_VERSION"))
        );
        if name == "atvprep-hook" {
            assert_eq!(Command::new(&path).output().unwrap().status.code(), Some(2));
        }
    }
}
