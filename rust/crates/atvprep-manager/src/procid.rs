//! Conservative process identity for deployment permits across PID reuse and namespaces.
use std::{
    collections::BTreeMap, io::Read, os::unix::fs::MetadataExt, path::Path, sync::OnceLock,
    time::Duration,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    Alive,
    Gone,
    Unknown,
}
enum Observation {
    Running(String),
    Gone,
    Unknown,
}

/// Parse Linux stat fields after the final closing parenthesis of the command name.
#[must_use]
pub fn parse_stat(text: &str) -> Option<(&str, &str)> {
    let fields: Vec<_> = text
        .rsplit_once(')')
        .map_or(text, |(_, rest)| rest)
        .split_whitespace()
        .collect();
    Some((*fields.first()?, *fields.get(19)?))
}

#[must_use]
pub fn host_name() -> String {
    rustix::system::uname()
        .nodename()
        .to_string_lossy()
        .chars()
        .take(128)
        .collect()
}

fn bounded_text(path: &Path) -> Option<String> {
    let mut value = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(16 * 1024)
        .read_to_string(&mut value)
        .ok()?;
    (value.len() < 16 * 1024).then_some(value)
}

fn command(program: &Path, args: &[String]) -> Option<(Option<i32>, String)> {
    let environment = BTreeMap::from([
        ("LC_ALL".to_owned(), "C".to_owned()),
        ("TZ".to_owned(), "UTC".to_owned()),
        (
            "PATH".to_owned(),
            "/usr/sbin:/sbin:/bin:/usr/bin".to_owned(),
        ),
    ]);
    let result = crate::worker::process::run(
        program,
        args,
        &environment,
        Duration::from_secs(10),
        Duration::from_millis(100),
        || false,
    )
    .ok()?;
    if result.reason != crate::worker::process::StopReason::Exited {
        return None;
    }
    Some((
        result.exit_code,
        String::from_utf8(result.output).ok()?.trim().to_owned(),
    ))
}

fn place() -> Option<String> {
    if let (Ok(namespace), Some(boot)) = (
        std::fs::metadata("/proc/self/ns/pid"),
        bounded_text(Path::new("/proc/sys/kernel/random/boot_id")),
    ) {
        let boot = boot.trim();
        if !boot.is_empty() {
            return Some(format!("linux:{}:{boot}", namespace.ino()));
        }
    }
    if cfg!(target_os = "macos") {
        let (code, boot) = command(
            Path::new("/usr/sbin/sysctl"),
            &["-n".to_owned(), "kern.boottime".to_owned()],
        )?;
        if code == Some(0) && !boot.is_empty() {
            return Some(format!("darwin:{boot}"));
        }
    }
    None
}

fn observe(pid: i64) -> Observation {
    let Some(pid) = i32::try_from(pid)
        .ok()
        .filter(|pid| *pid > 0)
        .and_then(rustix::process::Pid::from_raw)
    else {
        return Observation::Unknown;
    };
    if Path::new("/proc/self/stat").exists() {
        let path = format!("/proc/{}/stat", pid.as_raw_nonzero());
        match bounded_text(Path::new(&path)) {
            Some(text) => {
                let Some((state, started)) = parse_stat(&text) else {
                    return Observation::Unknown;
                };
                if matches!(state, "Z" | "X") {
                    Observation::Gone
                } else {
                    Observation::Running(format!("proc:{started}"))
                }
            }
            None if Path::new("/proc/1").exists()
                && rustix::process::test_kill_process(pid) == Err(rustix::io::Errno::SRCH) =>
            {
                Observation::Gone
            }
            None => Observation::Unknown,
        }
    } else {
        if rustix::process::test_kill_process(pid) == Err(rustix::io::Errno::SRCH) {
            return Observation::Gone;
        }
        let Some((code, started)) = command(
            Path::new("/bin/ps"),
            &[
                "-o".to_owned(),
                "lstart=".to_owned(),
                "-p".to_owned(),
                pid.as_raw_nonzero().to_string(),
            ],
        ) else {
            return Observation::Unknown;
        };
        if code == Some(0) && !started.is_empty() {
            Observation::Running(format!("ps:{started}"))
        } else {
            Observation::Unknown
        }
    }
}

/// Stable self-identity, or no identity when the host cannot safely establish it.
#[must_use]
pub fn identity_of_self() -> Option<String> {
    static IDENTITY: OnceLock<Option<String>> = OnceLock::new();
    IDENTITY
        .get_or_init(|| {
            let place = place()?;
            match observe(i64::from(std::process::id())) {
                Observation::Running(started) => Some(format!("{place}|{started}")),
                _ => None,
            }
        })
        .clone()
}

/// Only `Gone` permits automatic cleanup. Legacy or foreign records remain unknown.
#[must_use]
pub fn liveness(host: Option<&str>, pid: Option<i64>, recorded: Option<&str>) -> Liveness {
    let (Some(host), Some(pid), Some(recorded)) = (host, pid, recorded) else {
        return Liveness::Unknown;
    };
    if host != host_name() {
        return Liveness::Unknown;
    }
    let Some((where_recorded, started)) = recorded.rsplit_once('|') else {
        return Liveness::Unknown;
    };
    if !started.starts_with("proc:") && !started.starts_with("ps:")
        || place().as_deref() != Some(where_recorded)
    {
        return Liveness::Unknown;
    }
    match observe(pid) {
        Observation::Running(current) if current == started => Liveness::Alive,
        Observation::Gone | Observation::Running(_) => Liveness::Gone,
        Observation::Unknown => Liveness::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn proc_stat_parser_matches_python() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../testdata/golden/procid.json")).unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            assert_eq!(
                serde_json::to_value(parse_stat(case["input"].as_str().unwrap())).unwrap(),
                case["expected"]
            );
        }
    }
    #[test]
    fn observes_live_and_exited_processes_without_trusting_foreign_records() {
        let host = host_name();
        let recorded = identity_of_self().expect("supported host must expose a process identity");
        assert_eq!(
            liveness(
                Some(&host),
                Some(i64::from(std::process::id())),
                Some(&recorded)
            ),
            Liveness::Alive
        );
        assert_eq!(
            liveness(Some("foreign-host"), Some(1), Some(&recorded)),
            Liveness::Unknown
        );
        assert_eq!(
            liveness(Some(&host), None, Some(&recorded)),
            Liveness::Unknown
        );
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("10")
            .spawn()
            .unwrap();
        let pid = i64::from(child.id());
        let Observation::Running(started) = observe(pid) else {
            let _ = child.kill();
            panic!("child identity unavailable")
        };
        let identity = format!("{}|{started}", place().unwrap());
        assert_eq!(
            liveness(Some(&host), Some(pid), Some(&identity)),
            Liveness::Alive
        );
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(
            liveness(Some(&host), Some(pid), Some(&identity)),
            Liveness::Gone
        );
    }
}
