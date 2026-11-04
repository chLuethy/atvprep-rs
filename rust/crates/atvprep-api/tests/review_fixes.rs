//! Regression tests for review findings on startup validation.
use atvprep_api::{
    config::ManagerConfig,
    state::{AppState, StartupError},
};
use atvprep_manager::db::Db;

fn config() -> ManagerConfig {
    ManagerConfig {
        api_secret: Some("secret".to_owned()),
        allowed_origins: vec!["http://testserver".to_owned()],
        ..ManagerConfig::default()
    }
}

#[test]
fn session_lifetimes_outside_the_auth_service_ranges_fail_at_startup() {
    for adjust in [
        (|c: &mut ManagerConfig| c.session_absolute_ttl_seconds = 172_800) as fn(&mut _),
        |c| c.session_absolute_ttl_seconds = 0,
        |c| c.session_idle_ttl_seconds = 28_801,
        |c| c.csrf_ttl_seconds = 0,
        |c| c.csrf_ttl_seconds = 3_601,
    ] {
        let mut config = config();
        adjust(&mut config);
        let error = AppState::new(Db::memory().unwrap(), config).err();
        assert_eq!(error, Some(StartupError::InvalidSessionLifetime));
    }
    assert!(AppState::new(Db::memory().unwrap(), config()).is_ok());
}
