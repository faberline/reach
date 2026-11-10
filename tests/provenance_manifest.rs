//! The import manifest must be parseable and truthful. Offline; hashing uses
//! the macOS `shasum` tool, so the content check runs on macOS only.
use std::{fs, path::PathBuf, process::Command};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Rows of the form `| sha256 | source path | repo path | status |`.
fn manifest_rows() -> Vec<(String, String, String, String)> {
    let text = fs::read_to_string(root().join("docs/IMPORT-PROVENANCE.md")).unwrap();
    text.lines()
        .filter_map(|line| {
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            // A table row splits into ["", c1, c2, c3, c4, ""].
            if cells.len() == 6
                && cells[1].len() == 64
                && cells[1].chars().all(|c| c.is_ascii_hexdigit())
            {
                Some((
                    cells[1].to_string(),
                    cells[2].trim_matches('`').to_string(),
                    cells[3].trim_matches('`').to_string(),
                    cells[4].to_string(),
                ))
            } else {
                None
            }
        })
        .collect()
}

#[test]
fn manifest_lists_every_copied_file_with_valid_hash() {
    let rows = manifest_rows();
    assert!(rows.len() >= 80, "manifest has only {} rows", rows.len());
    for (sha, source, repo_path, status) in &rows {
        assert!(
            sha.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
            "sha256 must be lowercase hex: {source}"
        );
        assert!(
            status == "copied" || status == "modified",
            "bad status for {source}: {status}"
        );
        let file = root().join(repo_path);
        assert!(file.is_file(), "manifest path missing: {repo_path}");
        if status == "copied" && cfg!(target_os = "macos") {
            let out = Command::new("/usr/bin/shasum")
                .args(["-a", "256"])
                .arg(&file)
                .output()
                .unwrap();
            let actual = String::from_utf8(out.stdout).unwrap();
            assert_eq!(
                actual.split_whitespace().next().unwrap(),
                sha,
                "copied file changed: {repo_path}"
            );
        }
    }
}

#[test]
fn manifest_records_copy_time_and_exclusions() {
    let text = fs::read_to_string(root().join("docs/IMPORT-PROVENANCE.md")).unwrap();
    for needle in ["target/", "複製時間", "來源最新 mtime"] {
        assert!(
            text.contains(needle),
            "IMPORT-PROVENANCE.md lacks {needle:?}"
        );
    }
}
