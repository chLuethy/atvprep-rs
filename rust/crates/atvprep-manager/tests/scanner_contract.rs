use atvprep_manager::worker::scanner;
use serde_json::{Value, json};

#[test]
fn scanner_matches_python_predicates_and_descriptor_sidecars() {
    for case in atvprep_testkit::load_golden::<Value, Value>("scanner") {
        let input = &case.input;
        let observed = match input["kind"].as_str().unwrap() {
            "temporary" => json!(scanner::is_temporary(input["name"].as_str().unwrap())),
            "excluded" => {
                let excluded: Vec<String> =
                    serde_json::from_value(input["excluded"].clone()).unwrap();
                json!(scanner::is_excluded(
                    input["relative"].as_str().unwrap(),
                    input["name"].as_str().unwrap(),
                    &excluded
                ))
            }
            "sidecars" => {
                let temporary = tempfile::tempdir().unwrap();
                let root = temporary.path().canonicalize().unwrap();
                std::fs::create_dir(root.join("movies")).unwrap();
                for file in input["files"].as_array().unwrap() {
                    let path = root.join("movies").join(file["name"].as_str().unwrap());
                    std::fs::write(
                        &path,
                        vec![b'x'; usize::try_from(file["size"].as_u64().unwrap()).unwrap()],
                    )
                    .unwrap();
                    std::fs::File::options()
                        .write(true)
                        .open(path)
                        .unwrap()
                        .set_times(std::fs::FileTimes::new().set_modified(
                            std::time::UNIX_EPOCH
                                + std::time::Duration::from_nanos(
                                    file["mtime_ns"].as_u64().unwrap(),
                                ),
                        ))
                        .unwrap();
                }
                let seen =
                    scanner::observe(&root, "movies/film.mp4", 0, 1_800_000_000_000_000).unwrap();
                json!({"fingerprint":seen.meta.sidecar_fingerprint,"sidecars":seen.sidecars})
            }
            _ => panic!("unknown scanner case"),
        };
        assert_eq!(observed, case.expected, "{}", case.name);
    }
}
