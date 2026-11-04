//! Runtime configuration, compatible with `atvprep_manager.config`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

pub const DEFAULT_CONFIG_PATH: &str = "/etc/atvprep-manager/config.toml";
pub const DEFAULT_STATE_DIR: &str = "/var/lib/atvprep-manager";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagerConfig {
    pub library_roots: Vec<PathBuf>,
    pub state_dir: PathBuf,
    pub database_path: Option<PathBuf>,
    pub bind_host: String,
    pub bind_port: u16,
    pub allowed_origins: Vec<String>,
    pub scan_interval_seconds: u64,
    pub settle_seconds: u64,
    pub auto_fix_mp4: bool,
    pub auto_apply_new_sidecars: bool,
    pub auto_transcode: bool,
    pub max_workers: u32,
    pub max_ffmpeg_workers: u32,
    pub max_probe_workers: u32,
    pub atvprep_command: Vec<String>,
    pub atvprep_executable: Option<PathBuf>,
    pub tool_path: String,
    pub ffprobe_executable: Option<PathBuf>,
    pub fix_timeout_seconds: u64,
    pub fix_extra_args: Vec<String>,
    pub convert_extra_args: Vec<String>,
    pub quarantine_dir: Option<PathBuf>,
    pub log_tail_bytes: usize,
    pub master_token_file: Option<PathBuf>,
    pub session_absolute_ttl_seconds: u64,
    pub session_idle_ttl_seconds: u64,
    pub csrf_ttl_seconds: u64,
    pub session_cookie_secure: bool,
    pub api_secret: Option<String>,
    pub approval_secret: Option<String>,
    pub config_source: String,
}

impl Default for ManagerConfig {
    fn default() -> Self {
        Self {
            library_roots: vec![
                PathBuf::from("/mnt/media/Film"),
                PathBuf::from("/mnt/media/TV"),
            ],
            state_dir: PathBuf::from(DEFAULT_STATE_DIR),
            database_path: None,
            bind_host: "127.0.0.1".to_owned(),
            bind_port: 8765,
            allowed_origins: Vec::new(),
            scan_interval_seconds: 3600,
            settle_seconds: 120,
            auto_fix_mp4: false,
            auto_apply_new_sidecars: false,
            auto_transcode: false,
            max_workers: 1,
            max_ffmpeg_workers: 1,
            max_probe_workers: 2,
            atvprep_command: vec!["atvprep".to_owned()],
            atvprep_executable: None,
            tool_path: "/usr/local/bin:/usr/bin:/bin".to_owned(),
            ffprobe_executable: None,
            fix_timeout_seconds: 4 * 3600,
            fix_extra_args: Vec::new(),
            convert_extra_args: Vec::new(),
            quarantine_dir: None,
            log_tail_bytes: 16_384,
            master_token_file: None,
            session_absolute_ttl_seconds: 28_800,
            session_idle_ttl_seconds: 3600,
            csrf_ttl_seconds: 900,
            session_cookie_secure: true,
            api_secret: None,
            approval_secret: None,
            config_source: "default".to_owned(),
        }
    }
}

impl ManagerConfig {
    #[must_use]
    pub fn db_path(&self) -> PathBuf {
        self.database_path
            .clone()
            .unwrap_or_else(|| self.state_dir.join("state.db"))
    }

    #[must_use]
    pub fn job_log_dir(&self) -> PathBuf {
        self.state_dir.join("logs")
    }
}

#[derive(Debug)]
pub enum ConfigError {
    Io(io::Error),
    Toml(toml::de::Error),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "configuration I/O error: {error}"),
            Self::Toml(error) => write!(f, "invalid configuration: {error}"),
        }
    }
}
impl std::error::Error for ConfigError {}
impl From<io::Error> for ConfigError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}
impl From<toml::de::Error> for ConfigError {
    fn from(value: toml::de::Error) -> Self {
        Self::Toml(value)
    }
}

#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    #[serde(default)]
    server: Server,
    #[serde(default)]
    library: Library,
    #[serde(default)]
    automation: Automation,
    #[serde(default)]
    atvprep: Atvprep,
    #[serde(default)]
    storage: Storage,
    #[serde(default)]
    auth: Auth,
}
#[derive(Debug, Default, Deserialize)]
struct Server {
    bind_host: Option<String>,
    bind_port: Option<u16>,
    allowed_origins: Option<StringList>,
}
#[derive(Debug, Default, Deserialize)]
struct Library {
    roots: Option<PathList>,
}
#[derive(Debug, Default, Deserialize)]
struct Automation {
    scan_interval_seconds: Option<u64>,
    settle_seconds: Option<u64>,
    auto_fix_mp4: Option<Bool>,
    auto_apply_new_sidecars: Option<Bool>,
    auto_transcode: Option<Bool>,
    max_workers: Option<u32>,
    max_ffmpeg_workers: Option<u32>,
    max_probe_workers: Option<u32>,
}
#[derive(Debug, Default, Deserialize)]
struct Atvprep {
    command: Option<StringList>,
    executable: Option<PathBuf>,
    tool_path: Option<String>,
    ffprobe: Option<PathBuf>,
    fix_timeout_seconds: Option<u64>,
    fix_extra_args: Option<StringList>,
    convert_extra_args: Option<StringList>,
    quarantine_dir: Option<PathBuf>,
}
#[derive(Debug, Default, Deserialize)]
struct Storage {
    state_dir: Option<PathBuf>,
    database_path: Option<PathBuf>,
    log_tail_bytes: Option<usize>,
}
#[derive(Debug, Default, Deserialize)]
struct Auth {
    master_token_file: Option<PathBuf>,
    session_absolute_ttl_seconds: Option<u64>,
    session_idle_ttl_seconds: Option<u64>,
    csrf_ttl_seconds: Option<u64>,
    session_cookie_secure: Option<Bool>,
    api_secret: Option<String>,
    approval_secret: Option<String>,
}

#[derive(Debug)]
struct StringList(Vec<String>);
impl<'de> Deserialize<'de> for StringList {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = StringList;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a string or list of strings")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(StringList(
                    v.split_whitespace().map(str::to_owned).collect(),
                ))
            }
            fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Self::Value, E> {
                self.visit_str(&v)
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element::<String>()? {
                    values.push(value);
                }
                Ok(StringList(values))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}
#[derive(Debug)]
struct PathList(Vec<PathBuf>);
impl<'de> Deserialize<'de> for PathList {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let list = StringList::deserialize(deserializer)?;
        Ok(Self(list.0.into_iter().map(PathBuf::from).collect()))
    }
}
#[derive(Debug)]
struct Bool(bool);
impl<'de> Deserialize<'de> for Bool {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl serde::de::Visitor<'_> for Visitor {
            type Value = Bool;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a boolean or boolean string")
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Bool, E> {
                Ok(Bool(v))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Bool, E> {
                match v.trim().to_ascii_lowercase().as_str() {
                    "1" | "true" | "yes" | "on" => Ok(Bool(true)),
                    "0" | "false" | "no" | "off" | "" => Ok(Bool(false)),
                    _ => Err(E::custom(format!("invalid boolean string {v:?}"))),
                }
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

/// Load TOML configuration, or defaults when the path does not exist.
///
/// # Errors
///
/// Returns an error when an existing configuration cannot be read or parsed.
pub fn load_config(path: Option<&Path>) -> Result<ManagerConfig, ConfigError> {
    let path = path.map_or_else(|| PathBuf::from(DEFAULT_CONFIG_PATH), Path::to_path_buf);
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(ManagerConfig::default());
        }
        Err(error) => return Err(error.into()),
    };
    let input: FileConfig = toml::from_str(&content)?;
    let mut cfg = ManagerConfig {
        config_source: "config".to_owned(),
        ..ManagerConfig::default()
    };
    if let Some(value) = input.library.roots {
        cfg.library_roots = value.0;
    }
    if let Some(value) = input.storage.state_dir {
        cfg.state_dir = value;
    }
    cfg.database_path = input.storage.database_path;
    if let Some(value) = input.server.bind_host {
        cfg.bind_host = value;
    }
    if let Some(value) = input.server.bind_port {
        cfg.bind_port = value;
    }
    if let Some(value) = input.server.allowed_origins {
        cfg.allowed_origins = value.0;
    }
    if let Some(value) = input.automation.scan_interval_seconds {
        cfg.scan_interval_seconds = value;
    }
    if let Some(value) = input.automation.settle_seconds {
        cfg.settle_seconds = value;
    }
    if let Some(value) = input.automation.auto_fix_mp4 {
        cfg.auto_fix_mp4 = value.0;
    }
    if let Some(value) = input.automation.auto_apply_new_sidecars {
        cfg.auto_apply_new_sidecars = value.0;
    }
    if let Some(value) = input.automation.auto_transcode {
        cfg.auto_transcode = value.0;
    }
    if let Some(value) = input
        .automation
        .max_ffmpeg_workers
        .or(input.automation.max_workers)
    {
        cfg.max_ffmpeg_workers = value;
        cfg.max_workers = value;
    }
    if let Some(value) = input.automation.max_probe_workers {
        cfg.max_probe_workers = value;
    }
    if let Some(value) = input.atvprep.command {
        cfg.atvprep_command = value.0;
    }
    cfg.atvprep_executable = input.atvprep.executable;
    if let Some(value) = input.atvprep.tool_path {
        cfg.tool_path = value;
    }
    cfg.ffprobe_executable = input.atvprep.ffprobe;
    if let Some(value) = input.atvprep.fix_timeout_seconds {
        cfg.fix_timeout_seconds = value;
    }
    if let Some(value) = input.atvprep.fix_extra_args {
        cfg.fix_extra_args = value.0;
    }
    if let Some(value) = input.atvprep.convert_extra_args {
        cfg.convert_extra_args = value.0;
    }
    cfg.quarantine_dir = input.atvprep.quarantine_dir;
    if let Some(value) = input.storage.log_tail_bytes {
        cfg.log_tail_bytes = value;
    }
    cfg.master_token_file = input.auth.master_token_file;
    if let Some(value) = input.auth.session_absolute_ttl_seconds {
        cfg.session_absolute_ttl_seconds = value;
    }
    if let Some(value) = input.auth.session_idle_ttl_seconds {
        cfg.session_idle_ttl_seconds = value;
    }
    if let Some(value) = input.auth.csrf_ttl_seconds {
        cfg.csrf_ttl_seconds = value;
    }
    if let Some(value) = input.auth.session_cookie_secure {
        cfg.session_cookie_secure = value.0;
    }
    cfg.api_secret = input.auth.api_secret;
    cfg.approval_secret = input.auth.approval_secret;
    Ok(cfg)
}

/// Write the documented default TOML configuration.
///
/// # Errors
///
/// Returns an error when the parent directory or destination cannot be written.
pub fn write_default_config(path: &Path) -> Result<PathBuf, ConfigError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, include_str!("default-config.toml"))?;
    Ok(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_config_uses_defaults() {
        assert_eq!(
            load_config(Some(Path::new("/definitely/not/a/config.toml"))).unwrap(),
            ManagerConfig::default()
        );
    }
    #[test]
    fn config_loads_alias_and_string_lists() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(&path, "[server]\nallowed_origins = 'https://a https://b'\n[automation]\nmax_workers = 3\n[atvprep]\ncommand = 'atvprep --quiet'\n").unwrap();
        let config = load_config(Some(&path)).unwrap();
        assert_eq!(
            config.allowed_origins,
            vec!["https://a".to_owned(), "https://b".to_owned()]
        );
        assert_eq!(config.max_ffmpeg_workers, 3);
        assert_eq!(
            config.atvprep_command,
            vec!["atvprep".to_owned(), "--quiet".to_owned()]
        );
    }
    #[test]
    fn boolean_strings_are_validated() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(&path, "[auth]\nsession_cookie_secure = 'off'\n").unwrap();
        assert!(!load_config(Some(&path)).unwrap().session_cookie_secure);
        fs::write(&path, "[auth]\nsession_cookie_secure = 'ture'\n").unwrap();
        assert!(load_config(Some(&path)).is_err());
    }
}
