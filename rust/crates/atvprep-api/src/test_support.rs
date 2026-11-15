//! Shared setup of the in-crate HTTP tests.

use crate::config::ManagerConfig;
use crate::state::AppState;

pub(crate) fn state() -> AppState {
    let config = ManagerConfig {
        api_secret: Some("secret".to_owned()),
        allowed_origins: vec!["http://localhost".to_owned()],
        ..ManagerConfig::default()
    };
    AppState::new(atvprep_manager::db::Db::memory().unwrap(), config).unwrap()
}
