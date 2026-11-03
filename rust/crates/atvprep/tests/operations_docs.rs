//! Documented entry points must remain accepted by the shipped binary.
use std::process::Command;

#[test]
fn documented_commands_and_versions_match_the_release() {
    let binary = env!("CARGO_BIN_EXE_atvprep");
    for arguments in [
        vec!["version"],
        vec!["--version"],
        vec!["manager", "version"],
    ] {
        let result = Command::new(binary).args(arguments).output().unwrap();
        assert!(result.status.success());
        assert!(
            String::from_utf8(result.stdout)
                .unwrap()
                .trim()
                .ends_with(env!("CARGO_PKG_VERSION"))
        );
    }
    for (arguments, required) in [
        (
            vec!["manager", "api", "--help"],
            vec!["--config", "--web-dist"],
        ),
        (
            vec!["manager", "migrate", "--help"],
            vec!["--database", "--generation"],
        ),
        (
            vec!["deploy", "release-window", "--help"],
            vec![
                "--execute",
                "--services-stopped",
                "--imports-stopped",
                "--backup-dir",
                "--drain-timeout",
            ],
        ),
        (
            vec!["deploy", "rollback", "--help"],
            vec![
                "--manifest",
                "--signature",
                "--trusted-key",
                "--asset-file",
                "--destination",
            ],
        ),
        (
            vec!["deploy", "gate", "--help"],
            vec![
                "status",
                "permits",
                "history",
                "reap-permits",
                "abandon-permit",
            ],
        ),
    ] {
        let result = Command::new(binary).args(arguments).output().unwrap();
        assert!(result.status.success());
        let help = String::from_utf8(result.stdout).unwrap();
        for flag in required {
            assert!(help.contains(flag), "missing documented argument: {flag}");
        }
    }
    let guide = include_str!("../../../../docs/rust-cutover.md");
    for command in [
        "atvprep version",
        "atvprep manager version",
        "atvprep deploy release-window",
        "atvprep deploy rollback",
        "atvprep manager api",
    ] {
        assert!(guide.contains(command));
    }
}
