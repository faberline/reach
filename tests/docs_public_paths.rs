//! reach is a public repository: docs must not carry local paths or private project names.
//! Offline: only reads files.
use std::{fs, path::PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &std::path::Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// Built by concatenation so this file does not match its own scan.
fn forbidden() -> [String; 3] {
    [
        concat!("/Us", "ers/").to_string(),
        concat!("auto-sum", "moners").to_string(),
        concat!("sum", "moners").to_string(),
    ]
}

fn scanned_files() -> Vec<PathBuf> {
    let mut files = vec![root().join("README.md")];
    let mut docs: Vec<PathBuf> = fs::read_dir(root().join("docs"))
        .expect("docs directory")
        .map(|e| e.expect("docs entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .collect();
    docs.sort();
    files.extend(docs);
    files
}

#[test]
fn docs_and_readme_have_no_local_paths_or_private_project_names() {
    let forbidden = forbidden();
    let mut hits = Vec::new();
    for file in scanned_files() {
        for (n, line) in read(&file).lines().enumerate() {
            let lower = line.to_lowercase();
            if forbidden.iter().any(|f| lower.contains(&f.to_lowercase())) {
                hits.push(format!(
                    "{}:{}",
                    file.strip_prefix(root()).unwrap().display(),
                    n + 1
                ));
            }
        }
    }
    assert!(hits.is_empty(), "local or private names found at: {hits:?}");
}

#[test]
fn provenance_describes_source_without_naming_it() {
    let doc = read(&root().join("docs/IMPORT-PROVENANCE.md"));
    assert!(
        doc.contains("非公開"),
        "IMPORT-PROVENANCE.md must say the source is non-public"
    );
    for word in ["symlink", "符號連結"] {
        assert!(
            !doc.contains(word),
            "IMPORT-PROVENANCE.md still mentions {word:?}"
        );
    }
}

#[test]
fn readme_states_the_split_with_surface() {
    let readme = read(&root().join("README.md"));
    let section: Vec<&str> = readme
        .lines()
        .skip_while(|l| !l.starts_with("## 與 surface 的分工"))
        .skip(1)
        .take_while(|l| !l.starts_with("## "))
        .collect();
    assert!(
        !section.is_empty(),
        "README.md must have a `## 與 surface 的分工` section"
    );
    let text = section.join("\n");
    for needle in ["surface", "cu-agent", "不含", "faberline/surface"] {
        assert!(text.contains(needle), "the section must mention {needle}");
    }
    let lines = section.iter().filter(|l| !l.trim().is_empty()).count();
    assert!(lines <= 8, "the section must stay short, got {lines} lines");
}
