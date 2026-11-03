//! Shared, cloneable application state.

use std::sync::Arc;

use atvprep_manager::clock::{Clock, SystemClock};
use atvprep_manager::db::Db;

use crate::config::ManagerConfig;

pub use crate::ws::hub::EventHub;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub config: ManagerConfig,
    pub api_secret: Arc<str>,
    pub approval_secret: Arc<str>,
    pub event_hub: Arc<EventHub>,
    pub clock: Arc<dyn Clock>,
    pub readiness_probes: Arc<Vec<ReadinessProbe>>,
    pub web_dist: Option<std::path::PathBuf>,
    pub shutting_down: Arc<std::sync::atomic::AtomicBool>,
}

pub type ReadinessProbe = Arc<dyn Fn() -> bool + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupError {
    MissingApiSecret,
    MissingAllowedOrigins,
    InsecureCookiesOnPublicBind,
}

impl std::fmt::Display for StartupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingApiSecret => f.write_str("api_secret must be configured"),
            Self::MissingAllowedOrigins => {
                f.write_str("at least one allowed origin must be configured")
            }
            Self::InsecureCookiesOnPublicBind => {
                f.write_str("insecure session cookies require loopback binding")
            }
        }
    }
}
impl std::error::Error for StartupError {}

impl AppState {
    /// Build validated server state from its database and configuration.
    ///
    /// # Errors
    ///
    /// Returns an error for a missing API secret or allowed origin, or when
    /// insecure cookies are configured on a non-loopback bind address.
    pub fn new(db: Db, config: ManagerConfig) -> Result<Self, StartupError> {
        let api_secret = config
            .api_secret
            .clone()
            .ok_or(StartupError::MissingApiSecret)?;
        if config.allowed_origins.is_empty() {
            return Err(StartupError::MissingAllowedOrigins);
        }
        if !config.session_cookie_secure
            && !matches!(config.bind_host.as_str(), "127.0.0.1" | "localhost" | "::1")
        {
            return Err(StartupError::InsecureCookiesOnPublicBind);
        }
        let approval_secret = config
            .approval_secret
            .clone()
            .unwrap_or_else(|| api_secret.clone());
        Ok(Self {
            db,
            config,
            api_secret: Arc::from(api_secret),
            approval_secret: Arc::from(approval_secret),
            event_hub: Arc::new(EventHub::default()),
            clock: Arc::new(SystemClock),
            readiness_probes: Arc::new(Vec::new()),
            web_dist: None,
            shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }

    #[must_use]
    pub fn with_readiness_probes(mut self, probes: Vec<ReadinessProbe>) -> Self {
        self.readiness_probes = Arc::new(probes);
        self
    }

    #[must_use]
    pub fn with_web_dist(mut self, path: std::path::PathBuf) -> Self {
        self.web_dist = Some(path);
        self
    }
}
