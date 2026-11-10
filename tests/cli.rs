use std::{fs, process::Command};

#[test]
fn capabilities_never_claim_runtime_or_input_authority() {
    let result = Command::new(env!("CARGO_BIN_EXE_mac-computer-use"))
        .arg("capabilities")
        .output()
        .unwrap();
    assert!(result.status.success());
    let body: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(body["schema_version"], 1);
    assert_eq!(body["report_kind"], "static_implementation");
    assert_eq!(body["runtime_permissions_checked"], false);
    assert_eq!(body["target_checked"], false);
    assert_eq!(body["action_allowed"], false);
    for operation in [
        "desktop",
        "foreground",
        "background_window",
        "keyboard",
        "accessibility_action",
    ] {
        assert_eq!(body["input"][operation], "unsupported");
    }
    for condition in ["pointer_preservation", "focus_preservation"] {
        assert_eq!(body["input"][condition], "unverified");
    }
    assert_eq!(
        body["observation"]["accessibility_to_cg_window_binding"],
        "unverified"
    );
    assert_eq!(
        body["observation"]["target_gui_session_binding"],
        "unverified"
    );
    assert_eq!(
        body["observation"]["screen_to_window_coordinate_mapping"],
        "unverified"
    );
}

#[test]
fn unimplemented_action_is_explicitly_rejected() {
    let result = Command::new(env!("CARGO_BIN_EXE_mac-computer-use"))
        .arg("action")
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(3));
    let body: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(body["status"], "unsupported");
    assert_eq!(body["action_allowed"], false);
}

#[test]
fn invalid_identity_is_rejected_before_querying_the_system() {
    let result = Command::new(env!("CARGO_BIN_EXE_mac-computer-use"))
        .args([
            "capture",
            "--pid",
            "0",
            "--window",
            "12",
            "--output",
            "/tmp/not-written.png",
        ])
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&result.stderr).contains("must be positive"));
}

#[test]
fn existing_image_is_not_modified_or_claimed() {
    let directory =
        std::env::temp_dir().join(format!("mac-cu-cli-existing-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let image = directory.join("frame.png");
    fs::write(&image, b"existing image").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_mac-computer-use"))
        .args(["capture", "--pid", "42", "--window", "7", "--output"])
        .arg(&image)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    let body: serde_json::Value = serde_json::from_slice(&result.stderr).unwrap();
    assert_eq!(body["action_allowed"], false);
    assert!(body["error"].as_str().unwrap().contains("already exists"));
    assert_eq!(fs::read(&image).unwrap(), b"existing image");
    assert!(!directory.join("frame.png.json").exists());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn unavailable_process_failure_is_recorded_and_returns_failure() {
    let directory = std::env::temp_dir().join(format!("mac-cu-cli-failure-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let image = directory.join("frame.png");
    let result = Command::new(env!("CARGO_BIN_EXE_mac-computer-use"))
        .args([
            "capture",
            "--pid",
            "2147483647",
            "--window",
            "7",
            "--output",
        ])
        .arg(&image)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    let body: serde_json::Value = serde_json::from_slice(&result.stderr).unwrap();
    assert_eq!(body["status"], "error");
    assert_eq!(body["action_allowed"], false);
    assert_eq!(body["image_ownership"], "unverified");
    assert!(body["error"].as_str().unwrap().contains("identity"));
    let saved: serde_json::Value =
        serde_json::from_slice(&fs::read(directory.join("frame.png.json")).unwrap()).unwrap();
    assert_eq!(saved, body);
    assert!(!image.exists());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn unavailable_binding_is_unknown_and_non_success() {
    let result = Command::new(env!("CARGO_BIN_EXE_mac-computer-use"))
        .args(["observe-binding", "--pid", "2147483647", "--window", "7"])
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(4));
    let body: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(body["status"], "unknown");
    assert_eq!(body["failed_stage"], "identity_before");
    assert_eq!(body["action_allowed"], false);
}
