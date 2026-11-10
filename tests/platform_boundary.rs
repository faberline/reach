//! Offline checks for the platform boundary (`src/platform/`). They read
//! files only; they never touch macOS permissions or other applications.

use std::{
    fs,
    path::{Path, PathBuf},
};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .map(|e| e.expect("dir entry").path())
        .collect::<Vec<_>>();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn platform_neutral_sources_have_no_native_bindings() {
    let src = root().join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    let macos_dir = src.join("platform").join("macos");
    // Inline cfg blocks that are still to be moved, and the unix (not macOS
    // specific) process helper. See docs/PLATFORM.md.
    let allowed = [
        "display_inventory.rs",
        "native_event_preview.rs",
        "bounded_process.rs",
    ];
    let forbidden = [
        "#[link(".to_string(),
        concat!("extern ", "\"C\"").to_string(),
        "CoreFoundation".to_string(),
        "ImageIO".to_string(),
        "CoreGraphics".to_string(),
        "CommonCrypto".to_string(),
    ];
    let mut offenders = Vec::new();
    for file in files {
        if file.starts_with(&macos_dir) || file.starts_with(src.join("bin")) {
            continue;
        }
        let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if file.parent() == Some(src.as_path()) && allowed.contains(&name) {
            continue;
        }
        let text = fs::read_to_string(&file).expect("read source");
        for word in &forbidden {
            if text.contains(word.as_str()) {
                offenders.push(format!("{} contains {word}", file.display()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "native bindings outside platform::macos: {offenders:#?}"
    );
}

#[test]
fn macos_adapter_is_only_declared_under_cfg() {
    let text = fs::read_to_string(root().join("src/platform/mod.rs"))
        .expect("src/platform/mod.rs must exist");
    let lines = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with("//"))
        .collect::<Vec<_>>();
    let pos = lines
        .iter()
        .position(|l| *l == "pub mod macos;")
        .expect("platform/mod.rs must declare `pub mod macos;`");
    assert!(pos > 0 && lines[pos - 1] == "#[cfg(target_os = \"macos\")]");
}

#[test]
fn no_empty_platform_stubs() {
    let dir = root().join("src/platform");
    let mut names = fs::read_dir(&dir)
        .expect("src/platform must exist")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(names, ["macos", "mod.rs"]);
}

#[test]
fn old_flat_macos_modules_are_gone() {
    for old in [
        "src/macos.rs",
        "src/capture_lock.rs",
        "src/stream_image.rs",
        "src/stream_status.rs",
    ] {
        assert!(
            !root().join(old).exists(),
            "{old} must move under src/platform/macos/"
        );
    }
}

#[test]
fn platform_doc_states_boundary() {
    let text =
        fs::read_to_string(root().join("docs/PLATFORM.md")).expect("docs/PLATFORM.md must exist");
    for needle in [
        "目前只支援 macOS",
        "platform::macos",
        "Backend",
        "新增平台",
        "不提供其他平台的 stub",
    ] {
        assert!(
            text.contains(needle),
            "docs/PLATFORM.md must mention {needle}"
        );
    }
}
