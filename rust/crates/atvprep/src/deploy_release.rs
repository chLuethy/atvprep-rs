//! Signed release installation, update and explicitly requested rollback.
use crate::sink::SharedSink;
use atvprep_deploy::{
    Asset, Error, MAX_ASSET_BYTES, MAX_MANIFEST_BYTES, Manifest, Result,
    download::Downloader,
    install_asset,
    local::{installation_lock, read_file, trusted_key},
    parse_signed_manifest, require_upgrade, select_asset,
};
use clap::{Arg, Command, value_parser};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

pub fn command(name: &'static str) -> Command {
    Command::new(name)
        .about(
            "Install a signed release; updates reject downgrades, rollback explicitly permits them",
        )
        .arg(
            Arg::new("manifest")
                .long("manifest")
                .value_name("PATH_OR_HTTPS_URL")
                .required(true),
        )
        .arg(
            Arg::new("signature")
                .long("signature")
                .value_name("PATH_OR_HTTPS_URL")
                .required(true),
        )
        .arg(
            Arg::new("trusted-key")
                .long("trusted-key")
                .value_name("PATH")
                .required(true),
        )
        .arg(
            Arg::new("destination")
                .long("destination")
                .value_name("PATH")
                .required(true),
        )
        .arg(Arg::new("asset-file").long("asset-file").value_name("PATH"))
        .arg(Arg::new("platform").long("platform").value_name("PLATFORM"))
        .arg(
            Arg::new("timeout")
                .long("timeout")
                .value_parser(value_parser!(u64).range(1..=300))
                .default_value("60"),
        )
}

fn field<'a>(matches: &'a clap::ArgMatches, name: &str) -> &'a str {
    matches
        .get_one::<String>(name)
        .expect("required release argument")
}

pub fn run(
    action: &str,
    matches: &clap::ArgMatches,
    sink: &SharedSink,
    stderr: &mut dyn Write,
) -> i32 {
    match install(action, matches) {
        Ok(manifest) => {
            let _ = writeln!(
                sink.clone(),
                "{}",
                serde_json::json!({"action":action,"version":manifest.version,"status":"installed"})
            );
            0
        }
        Err(error) => {
            let _ = writeln!(stderr, "deploy release: {error}");
            1
        }
    }
}

fn platform() -> Result<String> {
    let os = match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "macos",
        _ => {
            return Err(Error::Invalid(
                "unsupported installation platform".to_owned(),
            ));
        }
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        _ => {
            return Err(Error::Invalid(
                "unsupported installation architecture".to_owned(),
            ));
        }
    };
    Ok(format!("{os}-{arch}"))
}

fn binary_version(path: &Path) -> Result<String> {
    let executable = path
        .canonicalize()
        .map_err(|_| Error::Invalid("cannot resolve installed executable".to_owned()))?;
    let environment = BTreeMap::from([
        ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
        ("LANG".to_owned(), "C".to_owned()),
    ]);
    let result = atvprep_manager::worker::process::run(
        &executable,
        &["--version".to_owned()],
        &environment,
        Duration::from_secs(10),
        Duration::from_millis(100),
        || false,
    )
    .map_err(|_| Error::Invalid("installed executable failed version validation".to_owned()))?;
    if result.reason != atvprep_manager::worker::process::StopReason::Exited
        || result.exit_code != Some(0)
    {
        return Err(Error::Invalid(
            "installed executable failed version validation".to_owned(),
        ));
    }
    let text = std::str::from_utf8(&result.output).map_err(|_| {
        Error::Invalid("installed executable returned an invalid version".to_owned())
    })?;
    let words: Vec<_> = text.split_whitespace().collect();
    if words.len() != 2 || !matches!(words[0], "atvprep" | "atvprep-manager" | "atvprep-hook") {
        return Err(Error::Invalid(
            "installed executable returned an invalid version".to_owned(),
        ));
    }
    Ok(words[1].to_owned())
}

pub(crate) fn prepare(
    action: &str,
    matches: &clap::ArgMatches,
    execute: bool,
) -> Result<PreparedRelease> {
    let destination = PathBuf::from(field(matches, "destination"));
    let lock = if execute {
        Some(installation_lock(&destination)?)
    } else {
        None
    };
    let existing = match std::fs::symlink_metadata(&destination) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => {
            return Err(Error::Invalid(
                "cannot inspect installation destination".to_owned(),
            ));
        }
    };
    if action != "servarr"
        && ((action == "install" && existing) || (action != "install" && !existing))
    {
        return Err(Error::Invalid(
            "use install for a new destination and update or rollback for an existing installation"
                .to_owned(),
        ));
    }
    let key = trusted_key(Path::new(field(matches, "trusted-key")))?;
    let downloader = Downloader::new(Duration::from_secs(
        *matches.get_one::<u64>("timeout").expect("timeout default"),
    ))?;
    let location = field(matches, "manifest");
    let signature = field(matches, "signature");
    let manifest = if location.starts_with("https://") {
        downloader.manifest(location, signature, &key)?
    } else {
        parse_signed_manifest(
            &key,
            &read_file(Path::new(location), MAX_MANIFEST_BYTES as u64)?,
            &read_file(Path::new(signature), 64 * 1024)?,
        )?
    };
    let host = platform()?;
    let selected = matches
        .get_one::<String>("platform")
        .map_or(host.as_str(), String::as_str);
    if selected != host {
        return Err(Error::Invalid(
            "release platform does not match this host".to_owned(),
        ));
    }
    let asset = select_asset(&manifest, selected)?.clone();
    if action == "update" {
        require_upgrade(&manifest, &binary_version(&destination)?)?;
    }
    let same_hook_version = if action == "servarr" && existing {
        let installed = binary_version(&destination)?;
        if installed == manifest.version {
            true
        } else {
            require_upgrade(&manifest, &installed)?;
            false
        }
    } else {
        false
    };
    let body = if let Some(path) = matches.get_one::<String>("asset-file") {
        read_file(Path::new(path), asset.size)?
    } else {
        downloader.asset(&asset)?
    };
    let previous = if existing {
        Some(read_file(&destination, MAX_ASSET_BYTES)?)
    } else {
        None
    };
    atvprep_deploy::verify_asset(&asset, &body)?;
    if same_hook_version && previous.as_deref() != Some(body.as_slice()) {
        return Err(Error::Invalid(
            "installed hook version differs from the signed release bytes".to_owned(),
        ));
    }
    Ok(PreparedRelease {
        manifest,
        asset,
        body,
        destination,
        previous,
        host,
        lock,
    })
}

pub(crate) struct PreparedRelease {
    manifest: Manifest,
    asset: Asset,
    body: Vec<u8>,
    destination: PathBuf,
    previous: Option<Vec<u8>>,
    host: String,
    lock: Option<atvprep_deploy::local::InstallLock>,
}
impl PreparedRelease {
    pub fn stage(&self) -> Result<StagedRelease> {
        use std::os::unix::fs::OpenOptionsExt;
        if self.lock.is_none() {
            return Err(Error::Invalid(
                "staging requires the installation lock".to_owned(),
            ));
        }
        let parent = self
            .destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .canonicalize()
            .map_err(|_| Error::Invalid("cannot resolve staging directory".to_owned()))?;
        for suffix in 0..100_u8 {
            let path = parent.join(format!(".atvprep-stage-{}-{suffix}", std::process::id()));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
            {
                Ok(file) => {
                    drop(file);
                    let stage = StagedRelease { path };
                    install_asset(&self.asset, &self.body, &stage.path)?;
                    if !binary_version(&stage.path)
                        .is_ok_and(|version| version == self.manifest.version)
                    {
                        return Err(Error::Invalid(
                            "staged release failed version validation".to_owned(),
                        ));
                    }
                    return Ok(stage);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => {
                    return Err(Error::Invalid(
                        "cannot reserve staging executable".to_owned(),
                    ));
                }
            }
        }
        Err(Error::Invalid(
            "cannot reserve a unique staging executable".to_owned(),
        ))
    }
    pub fn version(&self) -> &str {
        &self.manifest.version
    }
    pub fn apply(self) -> Result<Manifest> {
        let Self {
            manifest,
            asset,
            body,
            destination,
            previous,
            host,
            lock,
        } = self;
        if lock.is_none() {
            return Err(Error::Invalid(
                "release was inspected without an installation lock".to_owned(),
            ));
        }
        if previous.as_deref() == Some(body.as_slice())
            && binary_version(&destination).is_ok_and(|version| version == manifest.version)
        {
            return Ok(manifest);
        }
        install_asset(&asset, &body, &destination)?;
        if !binary_version(&destination).is_ok_and(|version| version == manifest.version) {
            if let Some(previous) = previous {
                let backup = Asset {
                    platform: host,
                    url: "https://rollback.invalid/asset".to_owned(),
                    size: u64::try_from(previous.len()).unwrap_or(u64::MAX),
                    sha256: format!("{:x}", Sha256::digest(&previous)),
                };
                install_asset(&backup, &previous, &destination).map_err(|_| {
                    Error::Invalid(
                        "release validation and automatic rollback both failed".to_owned(),
                    )
                })?;
            } else {
                std::fs::remove_file(&destination).map_err(|_| {
                    Error::Invalid(
                    "release validation failed and the invalid installation could not be removed"
                        .to_owned(),
                )
                })?;
            }
            return Err(Error::Invalid(
                "release version validation failed; previous installation restored".to_owned(),
            ));
        }
        Ok(manifest)
    }
}

pub(crate) struct StagedRelease {
    path: PathBuf,
}
impl Drop for StagedRelease {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
impl StagedRelease {
    pub fn migrate(&self, database: &Path, generation: i64) -> Result<()> {
        let arguments = vec![
            "manager".to_owned(),
            "migrate".to_owned(),
            "--database".to_owned(),
            database.to_string_lossy().into_owned(),
            "--generation".to_owned(),
            generation.to_string(),
        ];
        let environment = BTreeMap::from([
            ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
            ("LANG".to_owned(), "C".to_owned()),
        ]);
        let process = atvprep_manager::worker::process::run(
            &self.path,
            &arguments,
            &environment,
            Duration::from_secs(300),
            Duration::from_secs(1),
            || false,
        )
        .map_err(|_| Error::Invalid("staged migration process failed".to_owned()))?;
        if process.reason != atvprep_manager::worker::process::StopReason::Exited
            || process.exit_code != Some(0)
        {
            return Err(Error::Invalid("staged migration process failed".to_owned()));
        }
        let document: serde_json::Value = serde_json::from_slice(&process.output)
            .map_err(|_| Error::Invalid("staged migration returned no valid result".to_owned()))?;
        if document["migrated"] != true
            || document["generation"].as_i64() != Some(generation)
            || document["schema_version"]
                .as_i64()
                .is_none_or(|version| version < 1)
        {
            return Err(Error::Invalid(
                "staged migration returned an invalid result".to_owned(),
            ));
        }
        Ok(())
    }
}

fn install(action: &str, matches: &clap::ArgMatches) -> Result<Manifest> {
    prepare(action, matches, true)?.apply()
}
