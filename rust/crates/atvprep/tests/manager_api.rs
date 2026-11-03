//! Process-level checks for the manager multicall entry point.
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

struct Server(Child);

#[test]
fn offline_migration_requires_closed_gate_and_exact_generation() {
    use atvprep_manager::{
        clock::SystemClock,
        db::Db,
        deployment_gate::{DeploymentGate, Operator},
        repo::Ctx,
        schema,
    };
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.sqlite");
    let db = Db::open(&path).unwrap();
    let mut conn = db.conn().unwrap();
    schema::migrate(&mut conn).unwrap();
    let invoke = |generation: i64| {
        Command::new(env!("CARGO_BIN_EXE_atvprep"))
            .args(["manager", "migrate", "--database"])
            .arg(&path)
            .arg("--generation")
            .arg(generation.to_string())
            .output()
            .unwrap()
    };
    assert!(!invoke(0).status.success());
    let operator = Operator::from_local_cli(&path).unwrap();
    let gate = DeploymentGate(Ctx {
        conn: conn.raw(),
        clock: &SystemClock,
    });
    let status = gate.close(&operator, "migration test").unwrap();
    let status = gate
        .begin_offline_migration(&operator, status.generation)
        .unwrap();
    assert!(!invoke(status.generation - 1).status.success());
    let result = invoke(status.generation);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["migrated"], true);
    assert_eq!(report["generation"], status.generation);
    assert_eq!(gate.status().unwrap().phase, "offline-migration");
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn manager(temp: &Path) -> PathBuf {
    let path = temp.join("atvprep-manager");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_atvprep"), &path).unwrap();
    path
}

fn config(temp: &Path) -> PathBuf {
    let path = temp.join("config.toml");
    let db = serde_json::to_string(&temp.join("state.db").to_string_lossy().to_string()).unwrap();
    std::fs::write(&path,format!("[server]\nbind_host = '127.0.0.1'\nbind_port = 0\nallowed_origins = ['http://testserver']\n[library]\nroots = []\n[storage]\ndatabase_path = {db}\n[auth]\napi_secret = 'test-secret'\nsession_cookie_secure = false\n")).unwrap();
    let probe = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|directory| directory.join("ffprobe"))
        .find(|path| path.is_file())
        .expect("ffprobe is required for manager integration tests")
        .canonicalize()
        .unwrap();
    let probe = serde_json::to_string(&probe.to_string_lossy()).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    writeln!(file, "[atvprep]\nffprobe = {probe}").unwrap();
    path
}

fn request(address: &str, request: &str) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

#[test]
fn manager_starts_on_an_ephemeral_port_bootstraps_and_stops_cleanly() {
    let temp = tempfile::tempdir().unwrap();
    let executable = manager(temp.path());
    let config = config(temp.path());
    let web = temp.path().join("web_dist");
    std::fs::create_dir(&web).unwrap();
    std::fs::write(web.join("index.html"), "frontend").unwrap();
    let child = Command::new(executable)
        .args(["api", "--config"])
        .arg(config)
        .arg("--web-dist")
        .arg(web)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut server = Server(child);
    let stdout = server.0.stdout.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let _ = reader.read_line(&mut line);
        let _ = sender.send(line);
    });
    let line = receiver
        .recv_timeout(Duration::from_secs(10))
        .expect("manager did not announce a listener");
    let Some(address) = line.trim().strip_prefix("Listening on ") else {
        let mut error = String::new();
        server
            .0
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut error)
            .unwrap();
        panic!("manager did not start: {error}");
    };
    let liveness = request(
        address,
        "GET /api/health/live HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert!(liveness.starts_with("HTTP/1.1 200"));
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let ready = request(
            address,
            "GET /api/health/ready HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        if ready.starts_with("HTTP/1.1 200") {
            break;
        }
        assert!(Instant::now() < deadline, "worker did not become ready");
        std::thread::sleep(Duration::from_millis(100));
    }
    let database = rusqlite::Connection::open(temp.path().join("state.db")).unwrap();
    database
        .execute(
            "UPDATE deployment_gate SET mode='maintenance' WHERE id=1",
            [],
        )
        .unwrap();
    assert!(
        request(
            address,
            "GET /api/health/ready HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        )
        .starts_with("HTTP/1.1 503")
    );
    let login = request(
        address,
        "POST /api/v1/auth/session HTTP/1.1\r\nHost: localhost\r\nOrigin: http://testserver\r\nAuthorization: Bearer test-secret\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    );
    assert!(login.starts_with("HTTP/1.1 200"));
    assert!(
        login
            .to_ascii_lowercase()
            .contains("set-cookie: atvprep_session=")
    );
    assert!(login.contains("csrf_token"));
    assert!(
        Command::new("/bin/kill")
            .args(["-TERM", &server.0.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = server.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < deadline, "manager did not stop");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn manager_refuses_existing_foreign_databases_without_modification() {
    let temp = tempfile::tempdir().unwrap();
    let executable = manager(temp.path());
    let config = config(temp.path());
    let db = temp.path().join("state.db");
    let connection = rusqlite::Connection::open(&db).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE foreign_data(value TEXT); INSERT INTO foreign_data VALUES('preserve');",
        )
        .unwrap();
    drop(connection);
    let output = Command::new(executable)
        .args(["api", "--config"])
        .arg(config)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let connection = rusqlite::Connection::open(&db).unwrap();
    let value: String = connection
        .query_row("SELECT value FROM foreign_data", [], |row| row.get(0))
        .unwrap();
    assert_eq!(value, "preserve");
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 0);
}

#[test]
fn manager_rejects_a_foreign_database_claiming_schema_version_one() {
    let temp = tempfile::tempdir().unwrap();
    let executable = manager(temp.path());
    let config = config(temp.path());
    let db = temp.path().join("state.db");
    let connection = rusqlite::Connection::open(&db).unwrap();
    connection
        .execute_batch("CREATE TABLE foreign_data(value TEXT); PRAGMA user_version=1;")
        .unwrap();
    drop(connection);
    let output = Command::new(executable)
        .args(["api", "--config"])
        .arg(config)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("not a Rust manager database")
    );
    let connection = rusqlite::Connection::open(db).unwrap();
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 1);
}

#[test]
fn manager_version_matches_the_multicall_entry_point() {
    let temp = tempfile::tempdir().unwrap();
    let executable = manager(temp.path());
    let multicall = Command::new(executable).arg("version").output().unwrap();
    let subcommand = Command::new(env!("CARGO_BIN_EXE_atvprep"))
        .args(["manager", "version"])
        .output()
        .unwrap();
    assert!(multicall.status.success());
    assert_eq!(multicall.stdout, subcommand.stdout);
    assert_eq!(
        String::from_utf8(multicall.stdout).unwrap().trim(),
        format!("atvprep-manager {}", env!("CARGO_PKG_VERSION"))
    );
}
