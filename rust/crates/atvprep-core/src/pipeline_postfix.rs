//! The post-mux compliance pipeline: apply postfix, verify, quarantine on a blocker (port of
//! `atvprep/pipeline_postfix.py`).

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::compliance::{Audit, Severity, Tools, Violation};
use crate::console::Console;
use crate::ffprobe::Ffprobe;
use crate::phase_timing::phase;
use crate::postfix::{PostfixError, bring_into_compliance};
use crate::pyjson::dumps_indent2;
use crate::stages::{Stage, StageReporter};
use crate::verify::{VerifyReport, verify_file_in};

/// Everything one run writes to and reads from: the human console, the stage reporter, the
/// external tools and ffprobe.
pub struct RunContext {
    pub console: Console,
    pub reporter: StageReporter,
    pub tools: Box<dyn Tools>,
    pub ffprobe: Ffprobe,
    /// The ffmpeg program to run instead of the plan's `ffmpeg` (a test seam).
    pub ffmpeg: Option<PathBuf>,
}

/// A free slot in `dir` for `name`: the name itself, else `<stem>-1<suffix>`, `-2`, ...
#[must_use]
pub fn resolve_quarantine_path(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    let suffix = path
        .extension()
        .map_or_else(String::new, |e| format!(".{}", e.to_string_lossy()));
    let mut i = 1_u32;
    loop {
        let candidate = dir.join(format!("{stem}-{i}{suffix}"));
        if !candidate.exists() {
            return candidate;
        }
        i += 1;
    }
}

#[derive(Serialize)]
struct SidecarPayload<'a> {
    path: String,
    passed: bool,
    violations: &'a [Violation],
}

/// The text of the `<quarantined>.verify.json` sidecar (Python `json.dumps(…, indent=2) + "\n"`).
#[must_use]
pub fn sidecar_json(original: &Path, report: &VerifyReport) -> String {
    let payload = SidecarPayload {
        path: original.to_string_lossy().into_owned(),
        passed: report.passed(),
        violations: &report.violations,
    };
    format!("{}\n", dumps_indent2(&payload))
}

/// Move `path` into `dir` as `name` (`-1`, `-2`, ... on collision) with a `.verify.json` sidecar
/// that records where the file came from: `path` itself, or the file `name` next to it when `path`
/// is a temporary output standing in for that file.
///
/// # Errors
///
/// I/O errors of creating the directory, moving the file or writing the sidecar.
pub fn quarantine_output(
    console: &mut Console,
    path: &Path,
    dir: &Path,
    name: &str,
    report: &VerifyReport,
) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let dest = resolve_quarantine_path(dir, name);
    let mut original = path.canonicalize()?;
    if path
        .file_name()
        .is_some_and(|n| n.to_string_lossy() != name)
    {
        original.set_file_name(name);
    }
    if let Err(error) = std::fs::rename(path, &dest) {
        // A quarantine directory on another filesystem cannot be reached by rename.
        if error.kind() != std::io::ErrorKind::CrossesDevices {
            return Err(error);
        }
        std::fs::copy(path, &dest)?;
        std::fs::File::open(&dest)?.sync_all()?;
        std::fs::remove_file(path)?;
    }
    let mut sidecar = dest.clone().into_os_string();
    sidecar.push(".verify.json");
    std::fs::write(PathBuf::from(sidecar), sidecar_json(&original, report))?;
    console.print(&format!(
        "  ✗ quarantined {} — {} blocker(s)",
        dest.display(),
        report.blockers().len()
    ));
    Ok(dest)
}

/// Apply the postfix rewrites, verify, and quarantine on a blocker.
///
/// A blocker with a quarantine directory moves the file there with its sidecar; without one the
/// blockers are listed and the file stays. The report is returned either way.
///
/// # Errors
///
/// [`PostfixError`] when a postfix rewrite cannot follow the file's layout or quarantining fails.
pub fn finalize_output(
    ctx: &mut RunContext,
    out_path: &Path,
    has_atmos: bool,
    quarantine_dir: Option<&Path>,
    verbose: bool,
) -> Result<VerifyReport, PostfixError> {
    ctx.reporter.stage(Stage::Finalize);
    let counters = {
        let _timing = phase("postfix", Some(out_path));
        bring_into_compliance(out_path, has_atmos, None, &ctx.ffprobe)?
    };
    if counters.ftyp_modified
        + counters.dec3_modified
        + counters.tkhd_modified
        + counters.hdlr_modified
        > 0
    {
        ctx.console.print(&format!(
            "  → postfix: ftyp={} dec3={} tkhd={} hdlr={}",
            counters.ftyp_modified,
            counters.dec3_modified,
            counters.tkhd_modified,
            counters.hdlr_modified
        ));
    }

    ctx.reporter.stage(Stage::Verify);
    let report = {
        let _timing = phase("verify", Some(out_path));
        let audit = Audit::new(ctx.tools.as_ref());
        let reporter = &mut ctx.reporter;
        let mut on_rule = |done: usize, total: usize| {
            #[allow(clippy::cast_precision_loss)]
            reporter.update(done as f64 / total as f64);
        };
        verify_file_in(&audit, out_path, None, Some(&mut on_rule))
    };
    if report.passed() {
        if verbose {
            ctx.console.print("  verified: ok");
        }
        return Ok(report);
    }

    let blockers = report.blockers();
    if let Some(dir) = quarantine_dir {
        let name = out_path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        quarantine_output(&mut ctx.console, out_path, dir, &name, &report)?;
        return Ok(report);
    }
    ctx.console.print(&format!(
        "  ✗ verify failed {} — {} blocker(s)",
        out_path.display(),
        blockers.len()
    ));
    for v in blockers {
        let track = v
            .track_id
            .map_or_else(String::new, |id| format!(" (track {id})"));
        debug_assert_eq!(v.severity, Severity::Blocker);
        ctx.console
            .print(&format!("    [blocker] {}{track}: {}", v.rule, v.message));
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;

    use atvprep_testkit::{fixture_path, load_golden, sha256_hex};
    use serde::Deserialize;
    use serde_json::{Value, json};

    use super::*;
    use crate::compliance::{ToolError, Tools, Violation};
    use crate::console::SharedBuffer;
    use crate::ffprobe::Ffprobe;
    use crate::stages::StageReporter;

    #[derive(Deserialize)]
    struct Ignored {}

    struct Recorded {
        info: String,
        dump: String,
        probe: Value,
    }

    impl Tools for Recorded {
        fn run_tool(&self, name: &str, _path: &Path) -> Result<String, ToolError> {
            Ok(if name == "mp4info" {
                self.info.clone()
            } else {
                self.dump.clone()
            })
        }

        fn ffprobe(&self, _path: &Path) -> Result<Value, ToolError> {
            Ok(self.probe.clone())
        }

        fn duplicate_audio(&self, _path: &Path, _probe: &Value) -> BTreeSet<i64> {
            BTreeSet::new()
        }
    }

    #[derive(Deserialize)]
    struct Input {
        kind: String,
        #[serde(default)]
        existing: Vec<String>,
        #[serde(default)]
        name: String,
        #[serde(default)]
        original: String,
        #[serde(default)]
        violations: Vec<(String, String, String, Option<u32>)>,
        #[serde(default)]
        file: String,
        #[serde(default)]
        atmos: bool,
        #[serde(default)]
        quarantine: bool,
        #[serde(default)]
        verbose: bool,
        #[serde(default)]
        info: String,
        #[serde(default)]
        dump: String,
        #[serde(default)]
        probe: Value,
    }

    fn violations_of(rows: &[(String, String, String, Option<u32>)]) -> Vec<Violation> {
        rows.iter()
            .map(|(rule, severity, message, track_id)| Violation {
                rule: rule.clone(),
                severity: if severity == "blocker" {
                    Severity::Blocker
                } else {
                    Severity::Warning
                },
                message: message.clone(),
                track_id: *track_id,
            })
            .collect()
    }

    /// A shell script that prints `probe` for every ffprobe call.
    fn fake_ffprobe(dir: &Path, probe: &Value) -> Ffprobe {
        use std::os::unix::fs::PermissionsExt;
        let json_file = dir.join("probe.json");
        std::fs::write(&json_file, probe.to_string()).unwrap();
        let script = dir.join("ffprobe");
        std::fs::write(
            &script,
            format!("#!/bin/sh\ncat '{}'\n", json_file.display()),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        Ffprobe::with_program(script)
    }

    fn normalise(text: &str, base: &Path) -> String {
        let real = base.canonicalize().unwrap();
        text.replace(real.to_str().unwrap(), "<base>")
            .replace(base.to_str().unwrap(), "<base>")
    }

    #[allow(
        clippy::too_many_lines,
        clippy::case_sensitive_file_extension_comparisons
    )]
    #[test]
    fn quarantine_and_finalize_match_the_python_oracle() {
        let texts: std::collections::BTreeMap<String, String> =
            load_golden::<Ignored, String>("compliance_texts")
                .into_iter()
                .map(|c| (c.name, c.expected))
                .collect();
        let cases = load_golden::<Input, Value>("pipeline_postfix");
        assert!(cases.len() >= 25);
        for case in cases {
            let input = &case.input;
            let name = &case.name;
            match input.kind.as_str() {
                "resolve" => {
                    let dir = tempfile::tempdir().unwrap();
                    for existing in &input.existing {
                        std::fs::write(dir.path().join(existing), "x").unwrap();
                    }
                    let found = resolve_quarantine_path(dir.path(), &input.name);
                    assert_eq!(
                        json!(found.file_name().unwrap().to_string_lossy()),
                        case.expected,
                        "{name}"
                    );
                }
                "sidecar" => {
                    let report = VerifyReport {
                        path: input.original.clone().into(),
                        violations: violations_of(&input.violations),
                    };
                    assert_eq!(
                        json!(sidecar_json(Path::new(&input.original), &report)),
                        case.expected,
                        "{name}"
                    );
                }
                _ => {
                    let base = tempfile::tempdir().unwrap();
                    let work = base.path().join("work");
                    std::fs::create_dir(&work).unwrap();
                    let target = work.join("out.mp4");
                    std::fs::copy(fixture_path(&input.file), &target).unwrap();
                    let quarantine = input.quarantine.then(|| base.path().join("q"));
                    let (console, printed) = Console::recording(false);
                    let machine = SharedBuffer::default();
                    let tools = Recorded {
                        info: texts[&input.info].clone(),
                        dump: texts[&input.dump].clone(),
                        probe: input.probe.clone(),
                    };
                    let mut ctx = RunContext {
                        ffmpeg: None,
                        console,
                        reporter: StageReporter::new(Box::new(machine.clone()), false),
                        tools: Box::new(tools),
                        ffprobe: fake_ffprobe(base.path(), &input.probe),
                    };
                    ctx.reporter.begin_file(false);
                    let report = finalize_output(
                        &mut ctx,
                        &target,
                        input.atmos,
                        quarantine.as_deref(),
                        input.verbose,
                    )
                    .unwrap();

                    let want = &case.expected;
                    let violations: Vec<Value> = report
                        .violations
                        .iter()
                        .map(|v| json!([v.rule, v.severity.as_str(), v.message, v.track_id]))
                        .collect();
                    assert_eq!(
                        Value::Array(violations),
                        want["violations"],
                        "{name} violations"
                    );
                    // Python's console treats `[blocker]` as markup and swallows it; Rust prints it.
                    let lines: Vec<String> = printed
                        .lines()
                        .iter()
                        .map(|l| normalise(&l.replace("[blocker]", ""), base.path()))
                        .collect();
                    assert_eq!(json!(lines), want["printed"], "{name} printed");
                    assert_eq!(
                        json!(machine.lines()),
                        want["machine"],
                        "{name} machine lines"
                    );

                    let mut listing: Vec<String> = Vec::new();
                    let mut sidecars = serde_json::Map::new();
                    let mut files = serde_json::Map::new();
                    for entry in walk(base.path()) {
                        let relative = entry
                            .strip_prefix(base.path())
                            .unwrap()
                            .to_string_lossy()
                            .into_owned();
                        if relative == "probe.json" || relative == "ffprobe" {
                            continue;
                        }
                        listing.push(relative.clone());
                        if relative.ends_with(".verify.json") {
                            let text = std::fs::read_to_string(&entry).unwrap();
                            sidecars.insert(relative.clone(), json!(normalise(&text, base.path())));
                        } else if relative.ends_with(".mp4") {
                            let file_name =
                                entry.file_name().unwrap().to_string_lossy().into_owned();
                            files.insert(
                                file_name,
                                json!(sha256_hex(&std::fs::read(&entry).unwrap())),
                            );
                        }
                    }
                    listing.sort();
                    assert_eq!(json!(listing), want["listing"], "{name} listing");
                    assert_eq!(Value::Object(sidecars), want["sidecars"], "{name} sidecars");
                    assert_eq!(Value::Object(files), want["files"], "{name} file hashes");
                }
            }
        }
    }

    fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                found.extend(walk(&path));
            } else {
                found.push(path);
            }
        }
        found
    }

    #[test]
    fn finalize_runs_postfix_before_verify() {
        let base = tempfile::tempdir().unwrap();
        let target = base.path().join("out.mp4");
        std::fs::copy(fixture_path("dby1.mp4"), &target).unwrap();
        let texts: std::collections::BTreeMap<String, String> =
            load_golden::<Ignored, String>("compliance_texts")
                .into_iter()
                .map(|c| (c.name, c.expected))
                .collect();
        let probe = atvprep_testkit::load_recorded("dby1.mp4").ffprobe.unwrap();
        let (console, printed) = Console::recording(false);
        let mut ctx = RunContext {
            ffmpeg: None,
            console,
            reporter: StageReporter::new(Box::new(SharedBuffer::default()), false),
            tools: Box::new(Recorded {
                info: texts["dby1.mp4 info recorded"].clone(),
                dump: texts["dby1.mp4 dump recorded"].clone(),
                probe: probe.clone(),
            }),
            ffprobe: fake_ffprobe(base.path(), &probe),
        };
        finalize_output(&mut ctx, &target, false, None, false).unwrap();
        assert!(
            printed
                .lines()
                .iter()
                .any(|l| l.starts_with("  → postfix: ftyp=1")),
            "{:?}",
            printed.lines()
        );
    }
}
