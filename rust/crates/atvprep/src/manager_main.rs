//! Manager API process entry point.
use atvprep_api::{auth::load_master_token, config::load_config, state::AppState};
use atvprep_manager::{db::Db, library::LibraryRoots, repo::Ctx, schema};
use clap::{Arg, Command, error::ErrorKind};
use rusqlite::{Connection, OpenFlags};
use std::{
    ffi::OsString,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

fn command() -> Command {
    Command::new("atvprep-manager")
        .version(env!("CARGO_PKG_VERSION"))
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(Command::new("version").about("Show the manager version"))
        .subcommand(
            Command::new("migrate")
                .about("Migrate a quiescent database inside an offline release window")
                .arg(Arg::new("database").long("database").required(true))
                .arg(
                    Arg::new("generation")
                        .long("generation")
                        .value_parser(clap::value_parser!(i64).range(0..))
                        .required(true),
                ),
        )
        .subcommand(
            Command::new("api")
                .about("Start the manager API")
                .arg(Arg::new("config").long("config").value_name("PATH"))
                .arg(Arg::new("web-dist").long("web-dist").value_name("PATH")),
        )
}

pub fn run(args: &[OsString]) -> u8 {
    let args = std::iter::once(OsString::from("atvprep-manager")).chain(args.iter().cloned());
    let matches = match command().try_get_matches_from(args) {
        Ok(matches) => matches,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            let _ = error.print();
            return 0;
        }
        Err(_) => {
            eprintln!("invalid manager arguments; use --help");
            return 2;
        }
    };
    match matches.subcommand() {
        Some(("migrate", args)) => {
            let path = Path::new(
                args.get_one::<String>("database")
                    .expect("database required"),
            );
            let generation = *args
                .get_one::<i64>("generation")
                .expect("generation required");
            match migrate(path, generation) {
                Ok(()) => {
                    println!(
                        "{}",
                        serde_json::json!({"migrated":true,"generation":generation,"schema_version":schema::VERSION})
                    );
                    0
                }
                Err(message) => {
                    eprintln!("manager: {message}");
                    1
                }
            }
        }
        Some(("version", _)) => {
            println!("atvprep-manager {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Some(("api", args)) => {
            let config = args.get_one::<String>("config").map(Path::new);
            let web_dist = args.get_one::<String>("web-dist").map(PathBuf::from);
            match start(config, web_dist) {
                Ok(()) => 0,
                Err(message) => {
                    eprintln!("manager: {message}");
                    1
                }
            }
        }
        _ => 2,
    }
}

fn migrate(path: &Path, generation: i64) -> Result<(), &'static str> {
    atvprep_manager::deployment_gate::Operator::from_local_cli(path)
        .map_err(|_| "migration requires the local database owner")?;
    let _lock = atvprep_deploy::local::migration_lock(path)
        .map_err(|_| "another migration is active or its directory is unsafe")?;
    verify_database(path)?;
    let db = Db::open(path).map_err(|_| "cannot open migration database")?;
    let mut conn = db.conn().map_err(|_| "cannot open migration connection")?;
    let check = |conn: &atvprep_manager::db::Conn| {
        let status = atvprep_manager::deployment_gate::DeploymentGate(Ctx {
            conn: conn.raw(),
            clock: &atvprep_manager::clock::SystemClock,
        })
        .status()
        .map_err(|_| "cannot inspect migration gate")?;
        if !status.healthy
            || status.mode != "maintenance"
            || status.phase != "offline-migration"
            || status.generation != generation
            || !status.quiesced
        {
            return Err("migration requires a quiescent offline gate at the expected generation");
        }
        Ok(())
    };
    check(&conn)?;
    conn.raw()
        .execute_batch("BEGIN EXCLUSIVE; ROLLBACK")
        .map_err(|_| "migration database has an active writer")?;
    schema::migrate(&mut conn).map_err(|_| "schema migration failed")?;
    check(&conn)
}

// A fresh Rust database is required. Inspect existing files without changing their SQLite mode.
fn verify_database(path: &Path) -> Result<(), &'static str> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err("cannot inspect database"),
    };
    if !metadata.file_type().is_file() {
        return Err("database must be a regular file");
    }
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| "cannot inspect database")?;
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|_| "cannot inspect database")?;
    if version == 0 {
        let objects: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
                [],
                |row| row.get(0),
            )
            .map_err(|_| "cannot inspect database")?;
        return if objects == 0 {
            Ok(())
        } else {
            Err("existing database is not an empty Rust database")
        };
    }
    // An older Rust schema is upgraded by `migrate`; it has the same tables.
    if !(1..=schema::VERSION).contains(&version) {
        return Err("unsupported database schema");
    }
    let mut stmt=conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name").map_err(|_|"cannot inspect database")?;
    let tables = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|_| "cannot inspect database")?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "cannot inspect database")?;
    if tables.iter().map(String::as_str).eq(schema::TABLES) {
        Ok(())
    } else {
        Err("existing database is not a Rust manager database")
    }
}

fn start(config_path: Option<&Path>, web_dist: Option<PathBuf>) -> Result<(), &'static str> {
    let mut config = load_config(config_path).map_err(|_| "cannot load configuration")?;
    if config.api_secret.as_deref().is_none_or(str::is_empty) {
        config.api_secret = if let Some(path) = &config.master_token_file {
            Some(load_master_token(path).map_err(|_| "cannot load master credential")?)
        } else {
            std::env::var("ATVPREP_API_SECRET")
                .ok()
                .filter(|value| !value.is_empty())
        };
    }
    // Validate security configuration before touching the configured database path.
    AppState::new(
        Db::memory().map_err(|_| "cannot validate configuration")?,
        config.clone(),
    )
    .map_err(|_| "invalid security configuration")?;
    let db_path = config.db_path();
    verify_database(&db_path)?;
    if let Some(parent) = db_path.parent().filter(|path| !path.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|_| "cannot prepare database directory")?;
    }
    let db = Db::open(&db_path).map_err(|_| "cannot open database")?;
    let mut conn = db.conn().map_err(|_| "cannot open database")?;
    schema::migrate(&mut conn).map_err(|_| "cannot initialize database")?;
    LibraryRoots(Ctx {
        conn: conn.raw(),
        clock: &atvprep_manager::clock::SystemClock,
    })
    .ensure_configured(&config.library_roots)
    .map_err(|_| "cannot register libraries")?;
    // An explicit directory wins, then a `web_dist` next to the executable, then the frontend
    // compiled into the binary.
    let beside_executable = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join("web_dist")));
    let web_dist = web_dist.or_else(|| {
        beside_executable
            .clone()
            .filter(|path| path.is_dir() || !atvprep_api::spa::embedded_available())
    });
    let bind = (config.bind_host.clone(), config.bind_port);
    let mut state = AppState::new(db, config).map_err(|_| "invalid security configuration")?;
    state = match web_dist {
        Some(directory) => state.with_web_dist(directory),
        None => state.with_embedded_web(),
    };
    let worker = start_worker(&state)?;
    let worker_probe = worker.clone();
    let database = state.db.clone();
    let clock = state.clock.clone();
    let state = state.with_readiness_probes(vec![
        Arc::new(move || worker_probe.as_ref().is_some_and(|worker| worker.healthy())),
        Arc::new(move || {
            database.conn().is_ok_and(|conn| {
                atvprep_manager::deployment_gate::DeploymentGate(Ctx {
                    conn: conn.raw(),
                    clock: clock.as_ref(),
                })
                .status()
                .is_ok_and(|gate| gate.healthy && gate.mode == "open" && gate.phase == "steady")
            })
        }),
    ]);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|_| "cannot initialize runtime")?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(bind)
            .await
            .map_err(|_| "cannot bind API listener")?;
        let address = listener
            .local_addr()
            .map_err(|_| "cannot inspect API listener")?;
        println!("Listening on {address}");
        let _ = std::io::stdout().flush();
        let shutting_down = state.shutting_down.clone();
        axum::serve(listener, atvprep_api::router(state))
            .with_graceful_shutdown(async move {
                shutdown().await;
                shutting_down.store(true, std::sync::atomic::Ordering::Release);
                if let Some(worker) = worker {
                    worker.request_stop();
                }
            })
            .await
            .map_err(|_| "API server failed")
    })
}

fn start_worker(
    state: &AppState,
) -> Result<Option<Arc<atvprep_manager::worker::runtime::WorkerHandle>>, &'static str> {
    use atvprep_manager::worker::{
        execution::ExecutionConfig,
        runtime::{RuntimeConfig, WorkerHandle},
        scan_service::ScanOptions,
        service::WorkerLimits,
    };
    let config = &state.config;
    let ffprobe = if let Some(path) = &config.ffprobe_executable {
        Some(path.clone())
    } else {
        std::env::split_paths(&config.tool_path)
            .map(|directory| directory.join("ffprobe"))
            .find(|path| path.is_file())
    };
    let Some(ffprobe) = ffprobe else {
        return Ok(None);
    };
    let ffprobe = ffprobe
        .canonicalize()
        .map_err(|_| "cannot resolve ffprobe executable")?;
    let executable = config
        .atvprep_executable
        .clone()
        .map_or_else(std::env::current_exe, Ok)
        .map_err(|_| "cannot resolve media executable")?
        .canonicalize()
        .map_err(|_| "cannot resolve media executable")?;
    atvprep_manager::worker::executor::validate_executable(&executable)
        .map_err(|_| "invalid media executable")?;
    atvprep_manager::worker::executor::validate_executable(&ffprobe)
        .map_err(|_| "invalid ffprobe executable")?;
    let approvals =
        atvprep_manager::approval::ApprovalVerifier::new(state.approval_secret.as_bytes())
            .map(Arc::new);
    let runtime = RuntimeConfig {
        owner: format!("worker-{}-{}", std::process::id(), state.clock.now_micros()),
        execution: ExecutionConfig {
            executable,
            environment: std::collections::BTreeMap::from([
                ("PATH".to_owned(), config.tool_path.clone()),
                ("LANG".to_owned(), "C".to_owned()),
            ]),
            timeout: std::time::Duration::from_secs(config.fix_timeout_seconds),
            grace: std::time::Duration::from_secs(2),
            lease_seconds: 30,
            shutdown: None,
            approvals,
            session_idle_seconds: i64::try_from(config.session_idle_ttl_seconds)
                .map_err(|_| "invalid session idle TTL")?,
        },
        ffprobe_executable: ffprobe,
        scan: ScanOptions {
            follow_mounts: false,
            excluded: vec![],
            settle_seconds: i64::try_from(config.settle_seconds)
                .map_err(|_| "invalid settle interval")?,
            lease_seconds: 30,
            shutdown: None,
        },
        scan_interval_seconds: i64::try_from(config.scan_interval_seconds)
            .map_err(|_| "invalid scan interval")?,
        maintenance: WorkerLimits::default(),
    };
    let worker: WorkerHandle =
        atvprep_manager::worker::runtime::start(&state.db, &state.clock, &runtime)
            .map_err(|_| "cannot start worker runtime")?;
    Ok(Some(Arc::new(worker)))
}

async fn shutdown() {
    if let Ok(mut signal) =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
    {
        tokio::select! {_ = tokio::signal::ctrl_c()=>{},_ = signal.recv()=>{}}
    } else {
        let _ = tokio::signal::ctrl_c().await;
    }
}
