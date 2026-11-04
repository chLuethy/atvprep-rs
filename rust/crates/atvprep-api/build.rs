//! Embeds the built frontend (`frontend/dist`) so a release binary can serve it without a `web_dist`
//! directory next to it. Without a build the list is empty and the binary needs `--web-dist`.
use std::{env, fmt::Write as _, fs, path::Path};

fn collect(root: &Path, dir: &Path, files: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            collect(root, &path, files);
        } else if kind.is_file()
            && let Ok(relative) = path.strip_prefix(root)
        {
            files.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
}

fn main() {
    let manifest = env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let dist = Path::new(&manifest).join("../../../frontend/dist");
    println!("cargo:rerun-if-changed={}", dist.display());
    let mut files = Vec::new();
    if let Ok(dist) = dist.canonicalize() {
        collect(&dist, &dist, &mut files);
        let mut code = String::from("pub static FILES: &[(&str, &[u8])] = &[\n");
        for relative in &files {
            let absolute = dist.join(relative);
            println!("cargo:rerun-if-changed={}", absolute.display());
            writeln!(
                code,
                "    ({relative:?}, include_bytes!({:?})),",
                absolute.display().to_string()
            )
            .expect("writing to a String cannot fail");
        }
        code.push_str("];\n");
        fs::write(
            Path::new(&env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("embedded_web.rs"),
            code,
        )
        .expect("cannot write the embedded frontend list");
    } else {
        fs::write(
            Path::new(&env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("embedded_web.rs"),
            "pub static FILES: &[(&str, &[u8])] = &[];\n",
        )
        .expect("cannot write the embedded frontend list");
    }
}
