//! Signed installation at a stable Servarr Custom Script path.
use crate::{deploy_release, sink::SharedSink};
use clap::{Arg, Command};
use std::{io::Write, path::Path};

pub fn command() -> Command {
    deploy_release::command("servarr")
        .about("Idempotently install or update a signed Rust hook at its stable Custom Script path")
        .arg(
            Arg::new("application")
                .long("application")
                .required(true)
                .value_parser(["sonarr", "radarr", "whisparr"]),
        )
}

pub fn run(matches: &clap::ArgMatches, sink: &SharedSink, stderr: &mut dyn Write) -> i32 {
    let destination = Path::new(
        matches
            .get_one::<String>("destination")
            .expect("required destination"),
    );
    if destination
        .file_name()
        .is_none_or(|leaf| leaf != "atvprep-hook")
    {
        let _ = writeln!(
            stderr,
            "Servarr destination must have the stable name atvprep-hook"
        );
        return 1;
    }
    deploy_release::run("servarr", matches, sink, stderr)
}
