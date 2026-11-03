//! `atvprep watch`: prepare new video files once they have stopped growing.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use atvprep_core::constants::VIDEO_EXTS;
use atvprep_core::pipeline_postfix::RunContext;
use atvprep_core::prepare::PrepareOptions;
use atvprep_core::prepare::paths::resolve;
use atvprep_core::prepare::pipeline::prepare_file;
use clap::ArgMatches;

use crate::Environment;
use crate::cli::{quarantine_dir, report};

/// How often the directory is looked at.
pub const POLL: Duration = Duration::from_secs(2);

fn is_video(path: &Path) -> bool {
    path.extension().is_some_and(|e| {
        let dotted = format!(".{}", e.to_string_lossy().to_lowercase());
        VIDEO_EXTS.contains(&dotted.as_str())
    })
}

fn videos_under(dir: &Path) -> HashSet<PathBuf> {
    let mut found = HashSet::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(path);
            } else if path.is_file() && is_video(&path) {
                found.insert(path);
            }
        }
    }
    found
}

/// What one look at the directory found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Poll {
    /// Files seen for the first time, sorted.
    pub detected: Vec<PathBuf>,
    /// Files that were stable for the settle time, sorted.
    pub ready: Vec<PathBuf>,
}

/// The files already there, the ones still growing, and the settle time.
pub struct Watcher {
    dir: PathBuf,
    known: HashSet<PathBuf>,
    pending: HashMap<PathBuf, (u64, Instant)>,
    settle: Duration,
}

impl Watcher {
    /// Start watching `dir`; what is in it now is never converted.
    #[must_use]
    pub fn seed(dir: &Path, settle: Duration) -> Self {
        Self {
            dir: dir.to_path_buf(),
            known: videos_under(dir),
            pending: HashMap::new(),
            settle,
        }
    }

    /// A file the run itself produced: it is not prepared again.
    pub fn mark_known(&mut self, path: PathBuf) {
        self.known.insert(path);
    }

    /// Look at the directory at time `now`.
    pub fn poll(&mut self, now: Instant) -> Poll {
        let size = |p: &Path| std::fs::metadata(p).map_or(0, |m| m.len());
        let mut fresh: Vec<PathBuf> = videos_under(&self.dir)
            .difference(&self.known)
            .filter(|f| !self.pending.contains_key(*f))
            .cloned()
            .collect();
        fresh.sort();
        for file in &fresh {
            self.pending.insert(file.clone(), (size(file), now));
        }
        let mut ready = Vec::new();
        for (file, (last_size, last_changed)) in self.pending.clone() {
            if !file.exists() {
                self.pending.remove(&file);
                continue;
            }
            let current = size(&file);
            if current != last_size {
                self.pending.insert(file, (current, now));
            } else if now.duration_since(last_changed) >= self.settle {
                self.pending.remove(&file);
                self.known.insert(file.clone());
                ready.push(file);
            }
        }
        ready.sort();
        Poll {
            detected: fresh,
            ready,
        }
    }
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

fn prepare_ready(
    ctx: &mut RunContext,
    env: &Environment,
    matches: &ArgMatches,
    out: Option<&Path>,
    file: &Path,
) -> Option<PathBuf> {
    ctx.console
        .print(&format!("\nPreparing: {}", name_of(file)));
    let stem = file
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    let options = PrepareOptions {
        output: out.map(|dir| dir.join(format!("{stem}.mp4"))),
        prefer_hevc10: matches.get_flag("prefer-hevc10"),
        crf: matches.get_one::<u32>("crf").copied().unwrap_or(18),
        keep_av1: matches.get_flag("keep-av1"),
        fix_dts: matches.get_flag("fix-dts"),
        quarantine_dir: quarantine_dir(matches, env),
        verbose: matches.get_flag("verbose"),
        ..PrepareOptions::default()
    };
    match prepare_file(ctx, file, &options, None) {
        Ok(result) => {
            report(ctx, &result);
            result.output
        }
        Err(error) => {
            ctx.console
                .print(&format!("Error processing {}: {error}", name_of(file)));
            None
        }
    }
}

/// Run until `stop` is set; `sleep` waits one poll interval. Exit code 0 after a stop, 1 for a
/// directory or output that cannot be used.
pub fn run(
    ctx: &mut RunContext,
    env: &Environment,
    matches: &ArgMatches,
    stop: &AtomicBool,
    sleep: &dyn Fn(Duration),
) -> i32 {
    let dir = resolve(Path::new(
        matches
            .get_one::<String>("directory")
            .map_or("", String::as_str),
    ));
    if !dir.is_dir() {
        ctx.console
            .print(&format!("Error: not a directory: {}", dir.display()));
        return 1;
    }
    let out = matches
        .get_one::<String>("output")
        .map(|o| resolve(Path::new(o)));
    if out
        .as_ref()
        .is_some_and(|o| o.extension().is_some_and(|e| e.eq_ignore_ascii_case("mp4")))
    {
        ctx.console
            .print("Error: --output must be a directory in watch mode, not a .mp4 path");
        return 1;
    }
    if let Some(out) = &out {
        let _ = std::fs::create_dir_all(out);
    }
    let settle = matches.get_one::<f64>("settle").copied().unwrap_or(10.0);
    ctx.console.print(&format!(
        "Watching {}  (settle {settle:.0}s, poll {:.0}s — Ctrl+C to stop)",
        dir.display(),
        POLL.as_secs_f64()
    ));
    let mut watcher = Watcher::seed(&dir, Duration::from_secs_f64(settle.max(0.0)));
    while !stop.load(Ordering::Relaxed) {
        sleep(POLL);
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let found = watcher.poll(Instant::now());
        for file in &found.detected {
            ctx.console.print(&format!("  detected: {}", name_of(file)));
        }
        for file in &found.ready {
            if let Some(output) = prepare_ready(ctx, env, matches, out.as_deref(), file) {
                watcher.mark_known(output);
            }
        }
    }
    ctx.console.print("\nWatch stopped.");
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(dir: &Path, name: &str, bytes: usize) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, vec![0_u8; bytes]).unwrap();
        path
    }

    const SETTLE: Duration = Duration::from_secs(10);

    #[test]
    fn a_new_file_is_prepared_after_it_settles() {
        let dir = tempfile::tempdir().unwrap();
        let mut watcher = Watcher::seed(dir.path(), SETTLE);
        let t0 = Instant::now();
        let file = video(dir.path(), "new.mkv", 10);
        assert_eq!(
            watcher.poll(t0),
            Poll {
                detected: vec![file.clone()],
                ready: vec![]
            }
        );
        assert_eq!(watcher.poll(t0 + Duration::from_secs(9)), Poll::default());
        assert_eq!(
            watcher.poll(t0 + SETTLE),
            Poll {
                detected: vec![],
                ready: vec![file]
            }
        );
        assert_eq!(watcher.poll(t0 + SETTLE * 2), Poll::default());
    }

    #[test]
    fn a_growing_file_restarts_the_wait() {
        let dir = tempfile::tempdir().unwrap();
        let mut watcher = Watcher::seed(dir.path(), SETTLE);
        let t0 = Instant::now();
        let file = video(dir.path(), "grow.mkv", 10);
        watcher.poll(t0);
        video(dir.path(), "grow.mkv", 20);
        assert_eq!(
            watcher.poll(t0 + Duration::from_secs(8)).ready,
            Vec::<PathBuf>::new()
        );
        assert_eq!(
            watcher.poll(t0 + Duration::from_secs(17)).ready,
            Vec::<PathBuf>::new()
        );
        assert_eq!(watcher.poll(t0 + Duration::from_secs(18)).ready, vec![file]);
    }

    #[test]
    fn existing_files_are_not_converted_on_startup() {
        let dir = tempfile::tempdir().unwrap();
        video(dir.path(), "old.mkv", 10);
        let mut watcher = Watcher::seed(dir.path(), SETTLE);
        let t0 = Instant::now();
        assert_eq!(watcher.poll(t0), Poll::default());
        assert_eq!(watcher.poll(t0 + SETTLE * 3), Poll::default());
    }

    #[test]
    fn a_vanished_file_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let mut watcher = Watcher::seed(dir.path(), SETTLE);
        let t0 = Instant::now();
        let file = video(dir.path(), "gone.mkv", 10);
        watcher.poll(t0);
        std::fs::remove_file(file).unwrap();
        assert_eq!(watcher.poll(t0 + SETTLE).ready, Vec::<PathBuf>::new());
    }

    #[test]
    fn the_prepared_output_is_not_prepared_again() {
        let dir = tempfile::tempdir().unwrap();
        let mut watcher = Watcher::seed(dir.path(), SETTLE);
        let output = dir.path().join("made.mp4");
        watcher.mark_known(output.clone());
        video(dir.path(), "made.mp4", 10);
        assert_eq!(watcher.poll(Instant::now()), Poll::default());
    }

    #[test]
    fn only_video_extensions_count_regardless_of_case() {
        let dir = tempfile::tempdir().unwrap();
        let mut watcher = Watcher::seed(dir.path(), SETTLE);
        video(dir.path(), "notes.txt", 5);
        let shout = video(dir.path(), "SHOUT.MKV", 5);
        assert_eq!(watcher.poll(Instant::now()).detected, vec![shout]);
    }

    fn context() -> (RunContext, atvprep_core::console::SharedBuffer) {
        let (console, printed) = atvprep_core::console::Console::recording(false);
        let ctx = RunContext {
            console,
            reporter: atvprep_core::stages::StageReporter::new(
                Box::new(atvprep_core::console::SharedBuffer::default()),
                false,
            ),
            tools: Box::new(atvprep_core::compliance::SystemTools::new()),
            ffprobe: atvprep_core::ffprobe::Ffprobe::new(),
            ffmpeg: None,
        };
        (ctx, printed)
    }

    fn watch_matches(args: &[&str]) -> ArgMatches {
        let mut full = vec!["atvprep", "watch"];
        full.extend_from_slice(args);
        let matches = crate::args::command().try_get_matches_from(full).unwrap();
        matches.subcommand_matches("watch").unwrap().clone()
    }

    #[test]
    fn output_must_be_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let (mut ctx, printed) = context();
        let matches = watch_matches(&[dir.path().to_str().unwrap(), "-o", "x.MP4"]);
        let stop = AtomicBool::new(false);
        let code = run(
            &mut ctx,
            &Environment::from_process(),
            &matches,
            &stop,
            &|_| {},
        );
        assert_eq!(code, 1);
        assert_eq!(
            printed.lines(),
            ["Error: --output must be a directory in watch mode, not a .mp4 path"]
        );
        let missing = dir.path().join("missing");
        let matches = watch_matches(&[missing.to_str().unwrap()]);
        assert_eq!(
            run(
                &mut ctx,
                &Environment::from_process(),
                &matches,
                &stop,
                &|_| {}
            ),
            1
        );
    }

    #[test]
    fn the_stop_flag_ends_the_loop_with_exit_0() {
        let dir = tempfile::tempdir().unwrap();
        let (mut ctx, printed) = context();
        let matches = watch_matches(&[dir.path().to_str().unwrap(), "--settle", "0"]);
        let stop = AtomicBool::new(false);
        let polls = std::cell::Cell::new(0);
        let sleep = |_: Duration| {
            polls.set(polls.get() + 1);
            if polls.get() == 2 {
                stop.store(true, Ordering::Relaxed);
            }
        };
        let code = run(
            &mut ctx,
            &Environment::from_process(),
            &matches,
            &stop,
            &sleep,
        );
        assert_eq!(code, 0);
        assert_eq!(polls.get(), 2);
        let lines = printed.lines();
        assert!(lines[0].starts_with("Watching "));
        assert!(lines[0].ends_with("(settle 0s, poll 2s — Ctrl+C to stop)"));
        assert_eq!(lines.last().map(String::as_str), Some("Watch stopped."));
    }
}
