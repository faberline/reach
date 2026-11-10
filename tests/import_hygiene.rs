//! Guards the imported tree: no machine-specific paths, no links out of the
//! repository, no private assets. Offline, touches no macOS permission.
use std::{
    fs,
    path::{Path, PathBuf},
};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if name == "target" || name == ".git" {
            continue;
        }
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn all_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    walk(&root(), &mut files);
    files
}

#[test]
fn no_machine_specific_or_game_content() {
    // Built with concat! so this file does not match its own patterns.
    let forbidden = [
        concat!("/Us", "ers/"),
        concat!("Desk", "top/"),
        concat!("](", "../"),
        concat!("`.", "./screenshots/"),
        concat!("`.", "./benchmark/"),
    ];
    let extensions = ["md", "rs", "swift", "sh", "py", "toml"];
    let mut hits = Vec::new();
    for path in all_files() {
        let rel = path
            .strip_prefix(root())
            .unwrap()
            .to_string_lossy()
            .to_string();
        if rel == "tests/import_hygiene.rs" || rel == "docs/IMPORT-PROVENANCE.md" {
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !extensions.contains(&ext) {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap();
        for (number, line) in text.lines().enumerate() {
            for pattern in forbidden {
                if line.contains(pattern) {
                    hits.push(format!("{rel}:{}: {pattern}", number + 1));
                }
            }
        }
    }
    assert!(hits.is_empty(), "forbidden content:\n{}", hits.join("\n"));
}

#[test]
fn no_game_or_secret_assets() {
    let forbidden = ["png", "jpg", "jpeg", "mlmodel", "mlpackage", "sqlite", "db"];
    let hits: Vec<String> = all_files()
        .into_iter()
        .filter(|path| {
            path.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| forbidden.contains(&e))
        })
        .map(|path| path.display().to_string())
        .collect();
    assert!(hits.is_empty(), "forbidden assets: {hits:?}");
}
