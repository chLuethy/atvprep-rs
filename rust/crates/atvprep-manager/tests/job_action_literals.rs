//! The stored names of the actions (`fix`, `convert`) live in `JobAction` and the migrations only.
//! Every other place asks the type, so a new action or a changed stored form is one edit.

use std::fs;
use std::path::{Path, PathBuf};

/// Files that may spell the literals, each with the reason. Everything else asks `JobAction`.
const ALL: [&str; 4] = ["'fix'", "\"fix\"", "'convert'", "\"convert\""];
const FILTER: [&str; 1] = ["\"convert\""];

const ALLOWED: [(&str, &str, &[&str]); 7] = [
    (
        "atvprep-manager/src/state_machine.rs",
        "JobAction itself",
        &ALL,
    ),
    (
        "atvprep-manager/src/schema.rs",
        "migrations of the history (ADR 0008)",
        &ALL,
    ),
    (
        "atvprep-manager/src/file_state.rs",
        "`fix` and `convert` here are the media file's desired action and filter, not a job action",
        &ALL,
    ),
    (
        "atvprep-api/src/read_files.rs",
        "`convert` here is the file filter and its counts, not a job action",
        &FILTER,
    ),
    (
        "atvprep-api/src/read.rs",
        "`convert` here is the overview's file count, not a job action",
        &FILTER,
    ),
    (
        "atvprep/src/main.rs",
        "`fix` and `convert` here are the CLI's command aliases, not a stored action",
        &ALL,
    ),
    (
        "atvprep/src/args.rs",
        "`fix` and `convert` here are the CLI's command aliases, not a stored action",
        &ALL,
    ),
];

fn sources(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("directory is readable").flatten() {
        let path = entry.path();
        if path.is_dir() {
            sources(&path, found);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
}

/// The code of a file without its inline test module.
fn production(text: &str) -> &str {
    text.split("#[cfg(test)]").next().unwrap_or(text)
}

#[test]
fn stored_action_literals_stay_inside_the_job_action_type() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut offenders = Vec::new();
    for member in fs::read_dir(&crates)
        .expect("crates are readable")
        .flatten()
    {
        let src = member.path().join("src");
        if !src.is_dir() {
            continue;
        }
        let mut files = Vec::new();
        sources(&src, &mut files);
        for file in files {
            let relative = file
                .strip_prefix(&crates)
                .expect("file is below the crates")
                .to_string_lossy()
                .replace('\\', "/");
            let exempt: &[&str] = ALLOWED
                .iter()
                .find(|(allowed, _, _)| *allowed == relative)
                .map_or(&[], |(_, _, literals)| *literals);
            let text = fs::read_to_string(&file).expect("source is readable");
            for (number, line) in production(&text).lines().enumerate() {
                let code = line.split("//").next().unwrap_or(line);
                if ALL
                    .iter()
                    .any(|literal| code.contains(literal) && !exempt.contains(literal))
                {
                    offenders.push(format!("{relative}:{}: {}", number + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "a stored action literal outside JobAction:\n{}",
        offenders.join("\n")
    );
}
