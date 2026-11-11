//! Run the compliance rules against a file and report violations (port of `atvprep/verify.py`).
//!
//! A verify pass is read-only. One broken rule (missing tool, parse error) never short-circuits
//! the audit: it becomes a `blocker` violation and the other rules still report.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::compliance::{Audit, Rule, Severity, Tools, Violation, all_rules};

/// The result of running the compliance rules against one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyReport {
    pub path: PathBuf,
    pub violations: Vec<Violation>,
}

impl VerifyReport {
    #[must_use]
    pub fn blockers(&self) -> Vec<&Violation> {
        self.violations
            .iter()
            .filter(|v| v.severity == Severity::Blocker)
            .collect()
    }

    #[must_use]
    pub fn warnings(&self) -> Vec<&Violation> {
        self.violations
            .iter()
            .filter(|v| v.severity == Severity::Warning)
            .collect()
    }

    /// No blocker-severity violation.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.blockers().is_empty()
    }
}

/// Run every rule against `path` with a fresh [`Audit`] and collect the violations.
///
/// A rule that cannot reach a verdict (tool missing, parse error, unreadable file) becomes a
/// synthetic `blocker` named after the rule, `rule crashed: <reason>`, and the audit goes on.
/// `on_rule` hears `(done, total)` after every rule.
pub fn verify_file(
    path: &Path,
    tools: &dyn Tools,
    rules: Option<&[Box<dyn Rule>]>,
    on_rule: Option<&mut dyn FnMut(usize, usize)>,
) -> VerifyReport {
    verify_file_in(&Audit::new(tools), path, rules, on_rule)
}

/// [`verify_file`] sharing the tool outputs of an existing [`Audit`].
pub fn verify_file_in(
    audit: &Audit<'_>,
    path: &Path,
    rules: Option<&[Box<dyn Rule>]>,
    mut on_rule: Option<&mut dyn FnMut(usize, usize)>,
) -> VerifyReport {
    let default_rules;
    let rules = if let Some(rules) = rules {
        rules
    } else {
        default_rules = all_rules();
        &default_rules
    };
    let mut report = VerifyReport {
        path: path.to_path_buf(),
        violations: Vec::new(),
    };
    for (index, rule) in rules.iter().enumerate() {
        match rule.check(audit, path) {
            Ok(found) => report.violations.extend(found),
            Err(error) => report.violations.push(Violation {
                rule: rule.name().to_owned(),
                severity: Severity::Blocker,
                message: format!("rule crashed: {error}"),
                track_id: None,
            }),
        }
        if let Some(on_rule) = on_rule.as_deref_mut() {
            on_rule(index + 1, rules.len());
        }
    }
    report
}

fn suffix_of(path: &Path) -> String {
    path.extension().map_or_else(String::new, |e| {
        format!(".{}", e.to_string_lossy().to_lowercase())
    })
}

fn collect(dir: &Path, extensions: &BTreeSet<String>, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // A symlinked directory is not followed; a symlinked file counts as the file it points to.
        let is_real_dir = entry.file_type().is_ok_and(|t| t.is_dir());
        if is_real_dir {
            collect(&path, extensions, found);
        } else if path.is_file() && extensions.contains(&suffix_of(&path)) {
            found.push(path);
        }
    }
}

/// Media files under `root` whose extension matches any of `extensions` (case-insensitive).
///
/// `root` may be a single file. The result is sorted by path so reports are deterministic.
#[must_use]
pub fn iter_media_files(root: &Path, extensions: &[&str]) -> Vec<PathBuf> {
    let extensions: BTreeSet<String> = extensions.iter().map(|e| e.to_lowercase()).collect();
    if root.is_file() {
        return if extensions.contains(&suffix_of(root)) {
            vec![root.to_path_buf()]
        } else {
            Vec::new()
        };
    }
    let mut found = Vec::new();
    collect(root, &extensions, &mut found);
    found.sort();
    found
}

/// Verify every MP4-like file under `root` (`.mp4 .mov .m4v` unless `extensions` says otherwise),
/// one report per file in path order.
pub fn verify_tree(
    root: &Path,
    tools: &dyn Tools,
    extensions: Option<&[&str]>,
) -> Vec<VerifyReport> {
    verify_tree_with(root, tools, extensions, None)
}

/// [`verify_tree`] with `rules` instead of the built-in rule set.
pub fn verify_tree_with(
    root: &Path,
    tools: &dyn Tools,
    extensions: Option<&[&str]>,
    rules: Option<&[Box<dyn Rule>]>,
) -> Vec<VerifyReport> {
    let extensions = extensions.unwrap_or(&[".mp4", ".mov", ".m4v"]);
    iter_media_files(root, extensions)
        .iter()
        .map(|file| verify_file(file, tools, rules, None))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;

    use atvprep_testkit::{fixture_path, load_golden};
    use serde::Deserialize;
    use serde_json::{Value, json};

    use super::*;
    use crate::compliance::{ToolError, Tools};

    #[derive(Deserialize)]
    struct Ignored {}

    fn texts() -> std::collections::BTreeMap<String, String> {
        load_golden::<Ignored, String>("compliance_texts")
            .into_iter()
            .map(|c| (c.name, c.expected))
            .collect()
    }

    struct Recorded {
        info: Option<String>,
        dump: Option<String>,
        probe: Option<Value>,
        duplicates: BTreeSet<i64>,
    }

    impl Tools for Recorded {
        fn run_tool(&self, name: &str, _path: &Path) -> std::result::Result<String, ToolError> {
            let text = if name == "mp4info" {
                &self.info
            } else {
                &self.dump
            };
            match text.as_deref() {
                Some(text) if !text.is_empty() => Ok(text.to_owned()),
                _ => Err(ToolError(format!(
                    "{name} produced no output for <path> (exit=0): "
                ))),
            }
        }

        fn ffprobe(&self, _path: &Path) -> std::result::Result<Value, ToolError> {
            self.probe
                .clone()
                .ok_or_else(|| ToolError("ffprobe failed".to_owned()))
        }

        fn duplicate_audio(&self, _path: &Path, _probe: &Value) -> BTreeSet<i64> {
            self.duplicates.clone()
        }
    }

    #[derive(Deserialize)]
    struct Input {
        kind: String,
        file: Option<String>,
        info: Option<String>,
        dump: Option<String>,
        probe: Option<Value>,
        #[serde(default)]
        duplicates: BTreeSet<i64>,
        layout: Option<Value>,
        #[serde(default)]
        extensions: Vec<String>,
    }

    fn recorded(texts: &std::collections::BTreeMap<String, String>, input: &Input) -> Recorded {
        Recorded {
            info: input.info.as_ref().map(|k| texts[k].clone()),
            dump: input.dump.as_ref().map(|k| texts[k].clone()),
            probe: input.probe.clone(),
            duplicates: input.duplicates.clone(),
        }
    }

    fn violations_json(report: &VerifyReport) -> Value {
        json!(
            report
                .violations
                .iter()
                .map(|v| json!([v.rule, v.severity.as_str(), v.message, v.track_id]))
                .collect::<Vec<_>>()
        )
    }

    fn build_tree(layout: &Value, base: &Path) {
        let strings = |key: &str| -> Vec<String> {
            layout[key].as_array().map_or_else(Vec::new, |a| {
                a.iter().map(|v| v.as_str().unwrap().to_owned()).collect()
            })
        };
        let late: BTreeSet<&str> = ["z.mp4", "sub/c.mov", "a.mp4"].into_iter().collect();
        for rel in strings("files") {
            let target = base.join(&rel);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            let fixture = if late.contains(rel.as_str()) {
                "plain_moov_last.mp4"
            } else {
                "plain_faststart.mp4"
            };
            std::fs::copy(fixture_path(fixture), target).unwrap();
        }
        for rel in strings("dirs") {
            let _ = std::fs::create_dir(base.join(rel));
        }
        if let Some(pair) = layout.get("symlink_dir").and_then(Value::as_array) {
            std::os::unix::fs::symlink(
                base.join(pair[1].as_str().unwrap()),
                base.join(pair[0].as_str().unwrap()),
            )
            .unwrap();
        }
        if let Some(pair) = layout.get("symlink_file").and_then(Value::as_array) {
            std::os::unix::fs::symlink(
                base.join(pair[1].as_str().unwrap()),
                base.join(pair[0].as_str().unwrap()),
            )
            .unwrap();
        }
    }

    #[test]
    fn verify_matches_the_python_oracle() {
        let texts = texts();
        let cases = load_golden::<Input, Value>("verify");
        assert!(cases.len() >= 30);
        for case in cases {
            let input = &case.input;
            let tools = recorded(&texts, input);
            let name = &case.name;
            if input.kind == "file" {
                let path = fixture_path(input.file.as_deref().unwrap());
                let mut calls = Vec::new();
                let mut on_rule = |done: usize, total: usize| calls.push([done, total]);
                let report = verify_file(&path, &tools, None, Some(&mut on_rule));
                assert_eq!(
                    violations_json(&report),
                    case.expected["violations"],
                    "{name} violations"
                );
                assert_eq!(json!(calls), case.expected["calls"], "{name} progress");
                assert_eq!(
                    json!(report.passed()),
                    case.expected["passed"],
                    "{name} passed"
                );
                assert_eq!(
                    json!(report.blockers().len()),
                    case.expected["blockers"],
                    "{name}"
                );
                assert_eq!(
                    json!(report.warnings().len()),
                    case.expected["warnings"],
                    "{name}"
                );
            } else {
                let dir = tempfile::tempdir().unwrap();
                let layout = input.layout.as_ref().unwrap();
                build_tree(layout, dir.path());
                let extensions: Vec<&str> = input.extensions.iter().map(String::as_str).collect();
                let tools = Recorded {
                    info: Some(texts["plain_faststart.mp4 info recorded"].clone()),
                    dump: Some(texts["plain_faststart.mp4 dump recorded"].clone()),
                    probe: Some(load_probe()),
                    duplicates: BTreeSet::new(),
                };
                let root = dir.path().join(layout["root"].as_str().unwrap());
                let reports = verify_tree(&root, &tools, Some(&extensions));
                let actual: Vec<Value> = reports
                    .iter()
                    .map(|r| {
                        json!([
                            r.path.strip_prefix(dir.path()).unwrap().to_string_lossy(),
                            r.violations.len()
                        ])
                    })
                    .collect();
                assert_eq!(Value::Array(actual), case.expected, "{name}");
            }
        }
    }

    fn load_probe() -> Value {
        atvprep_testkit::load_recorded("plain_faststart.mp4")
            .ffprobe
            .unwrap()
    }

    #[test]
    fn one_crashing_rule_yields_one_blocker_and_later_rules_run() {
        let tools = Recorded {
            info: None,
            dump: None,
            probe: None,
            duplicates: BTreeSet::new(),
        };
        let mut calls = 0;
        let mut on_rule = |_: usize, _: usize| calls += 1;
        let report = verify_file(
            &fixture_path("plain_faststart.mp4"),
            &tools,
            None,
            Some(&mut on_rule),
        );
        assert_eq!(calls, 13, "progress counts every rule");
        let crashed: Vec<&Violation> = report
            .violations
            .iter()
            .filter(|v| v.message.starts_with("rule crashed: "))
            .collect();
        assert!(crashed.len() >= 3);
        assert!(crashed.iter().all(|v| v.severity == Severity::Blocker));
        assert!(!report.passed());
    }

    #[test]
    fn tree_order_is_sorted_and_case_insensitive_on_the_extension() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["b.mp4", "A.MP4", "c.txt"] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        let files = iter_media_files(dir.path(), &[".mp4"]);
        let names: Vec<String> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["A.MP4", "b.mp4"]);
    }

    #[test]
    fn verify_starts_each_tool_once_and_the_cache_does_not_change_the_verdict() {
        use std::cell::RefCell;

        struct Counting {
            inner: Recorded,
            calls: RefCell<Vec<String>>,
        }

        impl Tools for Counting {
            fn run_tool(&self, name: &str, path: &Path) -> std::result::Result<String, ToolError> {
                self.calls.borrow_mut().push(name.to_owned());
                self.inner.run_tool(name, path)
            }

            fn ffprobe(&self, path: &Path) -> std::result::Result<Value, ToolError> {
                self.calls.borrow_mut().push("ffprobe".to_owned());
                self.inner.ffprobe(path)
            }

            fn duplicate_audio(&self, path: &Path, probe: &Value) -> BTreeSet<i64> {
                self.calls.borrow_mut().push("duplicates".to_owned());
                self.inner.duplicate_audio(path, probe)
            }
        }

        let texts = texts();
        let path = fixture_path("stereo_surround_subs.mp4");
        let tools = Counting {
            inner: Recorded {
                info: Some(texts["stereo_surround_subs.mp4 info recorded"].clone()),
                dump: Some(texts["stereo_surround_subs.mp4 dump recorded"].clone()),
                probe: atvprep_testkit::load_recorded("stereo_surround_subs.mp4").ffprobe,
                duplicates: BTreeSet::new(),
            },
            calls: RefCell::default(),
        };
        let cached = verify_file(&path, &tools, None, None);
        let count = |name: &str| tools.calls.borrow().iter().filter(|c| *c == name).count();
        assert_eq!(
            (count("mp4info"), count("mp4dump"), count("ffprobe")),
            (1, 1, 1)
        );

        // Rule by rule, each with a fresh audit: nothing is reused, and the verdict is the same.
        let mut uncached = Vec::new();
        for rule in all_rules() {
            let audit = Audit::new(&tools);
            if let Ok(found) = rule.check(&audit, &path) {
                uncached.extend(found);
            }
        }
        let key = |v: &Violation| (v.rule.clone(), v.message.clone());
        let mut left: Vec<_> = cached.violations.iter().map(key).collect();
        let mut right: Vec<_> = uncached.iter().map(key).collect();
        left.sort();
        right.sort();
        assert_eq!(left, right);
    }
}
