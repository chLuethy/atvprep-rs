//! The port forbids `unsafe` in its own code; the workspace lints are the single place that says so.

use std::fs;
use std::path::PathBuf;

fn read(relative: &str) -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    fs::read_to_string(root.join(relative)).expect("manifest is readable")
}

#[test]
fn workspace_denies_unsafe_code() {
    let workspace = read("Cargo.toml");
    let lints = workspace
        .split("[workspace.lints.rust]")
        .nth(1)
        .expect("workspace declares rust lints");
    let section = lints.split("\n[").next().unwrap_or(lints);
    assert!(section.contains("unsafe_code = \"forbid\""));
}

#[test]
fn every_crate_inherits_the_workspace_lints() {
    for manifest in [
        "crates/atvprep-core/Cargo.toml",
        "crates/atvprep/Cargo.toml",
    ] {
        let text = read(manifest);
        let lints = text
            .split("[lints]")
            .nth(1)
            .unwrap_or_else(|| panic!("{manifest} has no [lints] table"));
        assert!(
            lints.trim_start().starts_with("workspace = true"),
            "{manifest}"
        );
    }
}
