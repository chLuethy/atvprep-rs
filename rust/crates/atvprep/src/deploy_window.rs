//! Explicitly executed, otherwise read-only offline release windows.
use crate::{deploy_release, sink::SharedSink};
use atvprep_deploy::{
    Error, Result,
    window::{self, WindowOptions},
};
use atvprep_manager::{
    clock::SystemClock,
    db::{Conn, Db},
    deployment_gate::Operator,
};
use clap::{Arg, ArgAction, Command, value_parser};
use std::{io::Write, path::PathBuf, time::Duration};

pub fn command() -> Command {
    deploy_release::command("release-window")
        .about(
            "Validate an offline release window; --execute performs it and leaves the gate closed",
        )
        .arg(Arg::new("database").long("database").required(true))
        .arg(Arg::new("backup-dir").long("backup-dir").required(true))
        .arg(
            Arg::new("drain-timeout")
                .long("drain-timeout")
                .value_parser(value_parser!(u64).range(1..=3600))
                .default_value("60"),
        )
        .arg(
            Arg::new("execute")
                .long("execute")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("services-stopped")
                .long("services-stopped")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("imports-stopped")
                .long("imports-stopped")
                .action(ArgAction::SetTrue),
        )
}

fn perform(matches: &clap::ArgMatches) -> Result<serde_json::Value> {
    let execute = matches.get_flag("execute");
    let database = PathBuf::from(
        matches
            .get_one::<String>("database")
            .expect("required database"),
    );
    let options = WindowOptions {
        execute,
        services_stopped: matches.get_flag("services-stopped"),
        imports_stopped: matches.get_flag("imports-stopped"),
        timeout: Duration::from_secs(
            *matches
                .get_one::<u64>("drain-timeout")
                .expect("default timeout"),
        ),
        backup_directory: PathBuf::from(
            matches
                .get_one::<String>("backup-dir")
                .expect("required backup directory"),
        ),
    };
    let invalid = || {
        Error::Invalid(
            "release window requires an existing database owned by the caller".to_owned(),
        )
    };
    let operator = Operator::from_local_cli(&database).map_err(|_| invalid())?;
    let readonly = Conn::open_read_only(&database).map_err(|_| invalid())?;
    window::inspect(&readonly, &SystemClock)?;
    if execute && (!options.services_stopped || !options.imports_stopped) {
        return Err(Error::Invalid(
            "execution requires --services-stopped and --imports-stopped".to_owned(),
        ));
    }
    let release = deploy_release::prepare("update", matches, execute)?;
    let version = release.version().to_owned();
    let report = if execute {
        drop(readonly);
        let staged = release.stage()?;
        let db = Db::open(&database).map_err(|_| invalid())?;
        let conn = db.conn().map_err(|_| invalid())?;
        window::run(
            &conn,
            &SystemClock,
            &operator,
            &options,
            |generation| staged.migrate(&database, generation),
            || release.apply().map(|_| ()),
        )?
    } else {
        window::run(
            &readonly,
            &SystemClock,
            &operator,
            &options,
            |_| Err(Error::Invalid("dry run cannot migrate".to_owned())),
            || Err(Error::Invalid("dry run cannot activate".to_owned())),
        )?
    };
    Ok(
        serde_json::json!({"version":version,"executed":report.executed,"generation":report.generation,"gate_closed":report.gate_closed,"backup_created":report.backup_created}),
    )
}

pub fn run(matches: &clap::ArgMatches, sink: &SharedSink, stderr: &mut dyn Write) -> i32 {
    match perform(matches) {
        Ok(report) => {
            let _ = writeln!(sink.clone(), "{report}");
            0
        }
        Err(error) => {
            let _ = writeln!(stderr, "release window: {error}");
            if matches!(error, Error::DrainTimeout) {
                2
            } else {
                1
            }
        }
    }
}
