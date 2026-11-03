//! The `atvprep` command line on top of `atvprep-core`: `prepare`, `patch`, `verify` and `watch`.

pub mod args;
pub mod cli;
mod deploy_release;
mod deploy_window;
pub mod hook_main;
mod servarr;
pub mod sink;
pub mod watch;

use std::ffi::OsString;
use std::io::{BufRead, Write};
use std::path::PathBuf;

use atvprep_core::compliance::{SystemTools, Tools};
use atvprep_core::console::Console;
use atvprep_core::ffprobe::Ffprobe;
use atvprep_core::pipeline_postfix::RunContext;
use atvprep_core::stages::StageReporter;
use atvprep_deploy::{GateCommand, execute_gate};
use atvprep_manager::clock::SystemClock;
use atvprep_manager::db::Db;
use atvprep_manager::deployment_gate::Operator;

use crate::sink::SharedSink;

/// The streams one run talks to.
pub struct Io {
    pub stdin: Box<dyn BufRead>,
    pub stdout: Box<dyn Write + Send>,
    pub stderr: Box<dyn Write + Send>,
    pub stdout_is_terminal: bool,
    pub stdin_is_terminal: bool,
}

/// What a run reads from the outside world besides its arguments.
pub struct Environment {
    /// `ATVPREP_QUARANTINE`.
    pub quarantine: Option<String>,
    pub make_tools: fn() -> Box<dyn Tools>,
    pub ffprobe: Ffprobe,
    /// Replaces the `ffmpeg` of the plan (a test seam).
    pub ffmpeg: Option<PathBuf>,
}

impl Environment {
    /// The real tools on `PATH` and the real process environment.
    #[must_use]
    pub fn from_process() -> Self {
        Self {
            quarantine: std::env::var("ATVPREP_QUARANTINE").ok(),
            make_tools: || Box::new(SystemTools::new()),
            ffprobe: Ffprobe::new(),
            ffmpeg: None,
        }
    }
}

/// Parse `args` (without the program name) and run the command; the return value is the exit code.
#[must_use]
pub fn run(args: &[OsString], env: &Environment, io: Io) -> i32 {
    let Io {
        mut stdin,
        stdout,
        mut stderr,
        stdout_is_terminal,
        stdin_is_terminal,
    } = io;
    let mut full = vec![OsString::from("atvprep")];
    full.extend_from_slice(args);
    let mut command = args::command();
    let matches = match command.clone().try_get_matches_from(full) {
        Ok(matches) => matches,
        Err(error) => {
            let code = error.exit_code();
            let text = error.render().to_string();
            // Help and version go to stdout and succeed; usage errors go to stderr with code 2.
            let sink = SharedSink::new(stdout);
            let _ = if code == 0 {
                sink.clone().write_all(text.as_bytes())
            } else {
                stderr.write_all(text.as_bytes())
            };
            return code;
        }
    };
    let sink = SharedSink::new(stdout);
    let Some((name, sub)) = matches.subcommand() else {
        let _ = sink
            .clone()
            .write_all(command.render_help().to_string().as_bytes());
        return 1;
    };
    if name == "deploy" {
        return run_deploy(sub, &sink, &mut *stderr);
    }
    if name == "version" {
        let _ = writeln!(sink.clone(), "atvprep {}", atvprep_core::VERSION);
        return 0;
    }
    let mut ctx = RunContext {
        console: Console::to(Box::new(sink.clone()), stdout_is_terminal),
        reporter: StageReporter::new(Box::new(sink.clone()), stdout_is_terminal),
        tools: (env.make_tools)(),
        ffprobe: env.ffprobe.clone(),
        ffmpeg: env.ffmpeg.clone(),
    };
    let code = match name {
        "patch" => cli::run_patch(&mut ctx, sub),
        "verify" => cli::run_verify(&mut ctx, &sink, sub),
        "watch" => cli::run_watch(&mut ctx, env, sub),
        _ => cli::run_prepare(&mut ctx, env, &mut *stdin, stdin_is_terminal, sub),
    };
    ctx.reporter.finish();
    code
}

fn run_deploy(matches: &clap::ArgMatches, sink: &SharedSink, stderr: &mut dyn Write) -> i32 {
    if let Some(("servarr", arguments)) = matches.subcommand() {
        return servarr::run(arguments, sink, stderr);
    }
    if let Some(("release-window", window)) = matches.subcommand() {
        return deploy_window::run(window, sink, stderr);
    }
    if let Some((action @ ("install" | "update" | "rollback"), release)) = matches.subcommand() {
        return deploy_release::run(action, release, sink, stderr);
    }
    let Some(("gate", gate)) = matches.subcommand() else {
        return 2;
    };
    let Some(action) = gate.get_one::<String>("action") else {
        return 2;
    };
    let Some(database) = gate.get_one::<String>("database") else {
        return 2;
    };
    if matches!(
        action.as_str(),
        "permits" | "reap-permits" | "abandon-permit" | "history"
    ) {
        return run_permits(action, gate, std::path::Path::new(database), sink, stderr);
    }
    let command = match action.as_str() {
        "status" => GateCommand::Status,
        "close" => GateCommand::Close {
            reason: gate
                .get_one::<String>("reason")
                .expect("clap requires --reason for gate close")
                .clone(),
        },
        "open" => GateCommand::Open {
            generation: *gate
                .get_one::<i64>("generation")
                .expect("clap requires --generation for gate open"),
        },
        "begin-migration" => GateCommand::BeginMigration {
            generation: *gate
                .get_one::<i64>("generation")
                .expect("clap requires --generation for gate begin-migration"),
        },
        "finish-migration" => GateCommand::FinishMigration {
            generation: *gate
                .get_one::<i64>("generation")
                .expect("clap requires --generation for gate finish-migration"),
        },
        _ => return 2,
    };
    let database = std::path::Path::new(database);
    if !database.is_file() {
        let _ = stderr.write_all(b"deploy gate: the database file does not exist\n");
        return 1;
    }
    let operator = if matches!(command, GateCommand::Status) {
        Ok(None)
    } else {
        Operator::from_local_cli(database).map(Some)
    };
    let outcome = operator.and_then(|operator| {
        Db::open(database)
            .and_then(|db| db.conn())
            .and_then(|conn| execute_gate(&conn, &SystemClock, operator.as_ref(), &command))
    });
    match outcome {
        Ok(status) => {
            let _ = sink.clone().write_all(
                format!(
                    "mode={} phase={} generation={} healthy={} permits={} quiesced={}\n",
                    status.mode,
                    status.phase,
                    status.generation,
                    status.healthy,
                    status.permits,
                    status.quiesced
                )
                .as_bytes(),
            );
            0
        }
        Err(error) => {
            let _ = stderr.write_all(format!("deploy gate: {error}\n").as_bytes());
            1
        }
    }
}

fn run_permits(
    action: &str,
    args: &clap::ArgMatches,
    database: &std::path::Path,
    sink: &SharedSink,
    stderr: &mut dyn Write,
) -> i32 {
    let result = (|| {
        if !database.is_file() {
            return Err(atvprep_manager::Error::Invalid(
                "database file does not exist".to_owned(),
            ));
        }
        let operator = Operator::from_local_cli(database)?;
        let db = Db::open(database)?;
        let conn = db.conn()?;
        let gate = atvprep_manager::deployment_gate::DeploymentGate(atvprep_manager::repo::Ctx {
            conn: conn.raw(),
            clock: &SystemClock,
        });
        match action {
            "history" => {
                let after = *args.get_one::<i64>("after").expect("history after default");
                let limit = *args.get_one::<i64>("limit").expect("history limit default");
                let rows = gate.history(after, limit)?;
                let next = rows.last().map(|row| row.id);
                let rows:Vec<_>=rows.into_iter().map(|row|serde_json::json!({"id":row.id,"generation":row.generation,"kind":row.kind,"actor_kind":row.actor_kind,"at":row.at})).collect();
                Ok(serde_json::json!({"data":rows,"meta":{"limit":limit,"after":next}}))
            }
            "permits" => {
                let limit = *args.get_one::<i64>("limit").expect("permit limit default");
                let rows = gate.open_permits(limit)?;
                let status = gate.status()?;
                let rows:Vec<_>=rows.into_iter().map(|row|serde_json::json!({"id":row.id,"kind":row.kind,"holder":atvprep_api::redact::redact_text(&row.holder),"pid":row.pid,"host":row.host.map(|host|atvprep_api::redact::redact_text(&host)),"identity_recorded":row.proc_start.is_some(),"issued_at":row.issued_at})).collect();
                Ok(
                    serde_json::json!({"data":rows,"meta":{"limit":limit,"truncated":status.permits>limit}}),
                )
            }
            "reap-permits" => Ok(serde_json::json!({"reaped":gate.reap_permits(&operator)?})),
            "abandon-permit" => {
                gate.abandon_permit(
                    *args
                        .get_one::<i64>("permit-id")
                        .expect("permit id required"),
                    &operator,
                    args.get_one::<String>("evidence")
                        .expect("evidence required"),
                )?;
                Ok(serde_json::json!({"abandoned":true}))
            }
            _ => Err(atvprep_manager::Error::Invalid(
                "unknown permit operation".to_owned(),
            )),
        }
    })();
    match result {
        Ok(body) => {
            let _ = writeln!(sink.clone(), "{body}");
            0
        }
        Err(error) => {
            let _ = writeln!(stderr, "deploy gate: {error}");
            1
        }
    }
}
