//! Offline checks of the CI workflow contract (aw docs/standards/ci.md).
//! They read files only.
use std::{fs, path::PathBuf};

const LINUX_TESTS: [&str; 11] = [
    "ax_incomplete_docs",
    "bin_platform_split",
    "calibration_docs",
    "ci_workflow",
    "docs_links",
    "docs_public_paths",
    "import_hygiene",
    "platform_boundary",
    "provenance_manifest",
    "virtual_input",
    "window_identity_docs",
];
const MACOS_ONLY_TESTS: [&str; 7] = [
    "cli",
    "cu_agent_cli",
    "focus_watch_cli",
    "headless_check_cli",
    "hidden_appkit_check_cli",
    "hidden_window_check_cli",
    "macos_event_memory",
];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn workflow() -> String {
    fs::read_to_string(root().join(".github/workflows/ci.yml"))
        .expect(".github/workflows/ci.yml must exist")
}

/// Top-level job ids: lines indented by two spaces that end with `:`.
fn job_ids(text: &str) -> Vec<String> {
    let mut in_jobs = false;
    let mut ids = Vec::new();
    for line in text.lines() {
        if line.starts_with("jobs:") {
            in_jobs = true;
            continue;
        }
        if !in_jobs {
            continue;
        }
        if !line.is_empty() && !line.starts_with(' ') && !line.starts_with('#') {
            break;
        }
        if let Some(rest) = line.strip_prefix("  ") {
            if !rest.starts_with(' ') && rest.ends_with(':') {
                ids.push(rest.trim_end_matches(':').to_string());
            }
        }
    }
    ids
}

/// The text of one job, from its id line up to the next job id line.
fn job_block(text: &str, id: &str) -> String {
    let header = format!("  {id}:");
    let mut out = String::new();
    let mut inside = false;
    for line in text.lines() {
        if line == header {
            inside = true;
            continue;
        }
        if inside {
            let is_next_job = line.starts_with("  ")
                && !line.starts_with("   ")
                && line.trim_end().ends_with(':')
                && !line.trim().is_empty();
            if is_next_job {
                break;
            }
            out.push_str(line);
            out.push('\n');
        }
    }
    assert!(inside, "job {id} not found");
    out
}

#[test]
fn workflow_has_fixed_name_and_triggers() {
    let text = workflow();
    for needle in [
        "name: ci\n",
        "pull_request:",
        "permissions:",
        "contents: read",
    ] {
        assert!(text.contains(needle), "ci.yml must contain {needle:?}");
    }
    let lower = text.to_lowercase();
    for forbidden in [
        concat!("pull_request", "_target"),
        "contents: write",
        "write-all",
        "self-hosted",
        concat!("secrets", "."),
    ] {
        assert!(
            !lower.contains(forbidden),
            "ci.yml must not contain {forbidden}"
        );
    }
}

#[test]
fn job_ids_follow_the_stages() {
    let ids = job_ids(&workflow());
    assert!(ids.iter().any(|i| i == "lint"), "missing lint job: {ids:?}");
    assert!(ids.iter().any(|i| i == "gate"), "missing gate job: {ids:?}");
    assert!(
        ids.iter().any(|i| i.starts_with("test-")),
        "missing test-* job: {ids:?}"
    );
    for id in &ids {
        assert!(
            id == "lint" || id == "gate" || id.starts_with("test-") || id.starts_with("e2e-"),
            "job id {id} is not a CI stage"
        );
    }
}

#[test]
fn lint_runs_exactly_the_fixed_commands() {
    let text = workflow();
    let block = job_block(&text, "lint");
    let runs = block
        .lines()
        .filter_map(|l| {
            l.trim()
                .strip_prefix("run:")
                .or_else(|| l.trim().strip_prefix("- run:"))
        })
        .map(str::trim)
        .collect::<Vec<_>>();
    assert_eq!(
        runs,
        [
            "cargo fmt --all -- --check",
            "cargo clippy --workspace --all-targets --locked -- -D warnings",
        ],
        "lint must run exactly the two fixed commands"
    );
}

#[test]
fn later_stages_chain_and_gate_collects_all() {
    let text = workflow();
    let ids = job_ids(&text);
    let tests = ids
        .iter()
        .filter(|i| i.starts_with("test-"))
        .collect::<Vec<_>>();
    for id in &tests {
        let block = job_block(&text, id);
        assert!(
            block.contains("needs: [lint]"),
            "{id} must have `needs: [lint]`"
        );
    }
    for id in ids.iter().filter(|i| i.starts_with("e2e-")) {
        let block = job_block(&text, id);
        assert!(
            block.contains("needs: [test-"),
            "{id} must need the test jobs"
        );
    }
    let gate = job_block(&text, "gate");
    assert!(
        gate.lines().any(|l| l == "    if: always()"),
        "gate must run `if: always()` at job level"
    );
    for check in [
        r#"jq -e '.lint.result == "success"'"#,
        r#"jq -e 'all(.[]; .result == "success" or .result == "skipped")'"#,
    ] {
        assert!(gate.contains(check), "gate must run {check}");
    }
    assert!(
        job_block(&text, "test-linux").contains("cargo test --locked --lib"),
        "test-linux must keep running the lib tests"
    );
    let needs = gate
        .lines()
        .find_map(|l| l.trim().strip_prefix("needs:"))
        .expect("gate must have needs");
    for id in ids.iter().filter(|i| *i != "gate") {
        assert!(
            needs
                .split(|c: char| !(c.is_alphanumeric() || c == '-'))
                .any(|w| w == id),
            "gate must need {id}: {needs}"
        );
    }
}

#[test]
fn actions_are_pinned_to_full_commit_sha() {
    let text = workflow();
    let mut count = 0;
    for line in text.lines() {
        let t = line.trim();
        let Some(rest) = t
            .strip_prefix("- uses:")
            .or_else(|| t.strip_prefix("uses:"))
        else {
            continue;
        };
        count += 1;
        let (action, comment) = rest.split_once('#').unwrap_or((rest, ""));
        assert!(!comment.trim().is_empty(), "missing version comment: {t}");
        let (name, sha) = action
            .trim()
            .split_once('@')
            .expect("action must have @ref");
        assert!(name.contains('/'), "bad action name: {t}");
        assert!(
            sha.len() == 40 && sha.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
            "action must be pinned to a full SHA: {t}"
        );
    }
    assert!(count > 0, "expected at least one action");
}

#[test]
fn cargo_commands_are_locked() {
    let text = workflow();
    let mut count = 0;
    for line in text.lines() {
        // Single-line `run: cargo …` values and lines inside `run: |` blocks.
        let t = line.trim();
        let cmd = t
            .strip_prefix("- run:")
            .or_else(|| t.strip_prefix("run:"))
            .unwrap_or(t)
            .trim();
        if cmd.starts_with("cargo ") && !cmd.starts_with("cargo fmt") {
            count += 1;
            assert!(cmd.contains("--locked"), "cargo must use --locked: {cmd}");
        }
    }
    assert!(count > 0, "expected cargo commands");
}

/// A plain (unquoted, non-block) `run:` value that contains `: ` or ` #` is
/// not valid YAML, and GitHub then creates no checks at all.
#[test]
fn run_values_are_valid_yaml_scalars() {
    let text = workflow();
    let mut count = 0;
    for line in text.lines() {
        let t = line.trim();
        let Some(value) = t.strip_prefix("- run:").or_else(|| t.strip_prefix("run:")) else {
            continue;
        };
        count += 1;
        let value = value.trim();
        if value.starts_with(['|', '>', '"', '\'']) {
            continue;
        }
        assert!(
            !value.contains(": ") && !value.contains(" #"),
            "plain run value is not valid YAML, use `run: |`: {t}"
        );
    }
    assert!(count > 0, "expected run steps");
}

#[test]
fn concurrency_cancels_only_pull_requests() {
    let text = workflow();
    assert!(
        text.contains("cancel-in-progress: ${{ github.event_name == 'pull_request' }}"),
        "concurrency must cancel only pull request runs"
    );
}

#[test]
fn ci_does_not_read_issue_context() {
    let text = workflow().to_lowercase();
    for forbidden in [
        "github.event.pull_request.body",
        "github.event.pull_request.title",
        "github.event.pull_request.labels",
        "github.event.issue",
        "github.event.comment",
    ] {
        assert!(
            !text.contains(forbidden),
            "ci.yml must not read {forbidden}"
        );
    }
}

#[test]
fn build_doc_states_ci_scope() {
    let text = fs::read_to_string(root().join("docs/BUILD.md")).expect("docs/BUILD.md");
    for needle in ["## CI", "ubuntu-latest", "macos-latest", "gate", "#[ignore"] {
        assert!(text.contains(needle), "docs/BUILD.md must mention {needle}");
    }
    for needle in MACOS_ONLY_TESTS
        .into_iter()
        .chain(LINUX_TESTS)
        .chain(["--workspace --all-targets", "CARGO_BIN_EXE"])
    {
        assert!(text.contains(needle), "docs/BUILD.md must mention {needle}");
    }
}

/// Names after `--test` in a job block.
fn test_flags(block: &str) -> Vec<String> {
    let words = block.split_whitespace().collect::<Vec<_>>();
    words
        .windows(2)
        .filter(|w| w[0] == "--test")
        .map(|w| w[1].to_string())
        .collect()
}

#[test]
fn linux_runs_the_offline_integration_tests() {
    let block = job_block(&workflow(), "test-linux");
    assert!(
        block.contains("cargo test --locked --lib"),
        "test-linux must keep running the lib tests"
    );
    let flags = test_flags(&block);
    for name in LINUX_TESTS {
        assert!(
            flags.iter().any(|f| f == name),
            "test-linux must run `--test {name}`"
        );
    }
    for name in MACOS_ONLY_TESTS {
        assert!(
            !flags.iter().any(|f| f == name),
            "test-linux must not run the macOS-only test {name}"
        );
    }
    assert!(
        !block.contains("--workspace"),
        "test-linux must not run the whole workspace"
    );
}

#[test]
fn every_integration_test_is_classified_and_linux_ones_are_offline() {
    let mut found = fs::read_dir(root().join("tests"))
        .expect("tests directory")
        .filter_map(|e| {
            let name = e.ok()?.file_name().to_string_lossy().into_owned();
            name.strip_suffix(".rs").map(str::to_string)
        })
        .collect::<Vec<_>>();
    found.sort();
    let mut expected = LINUX_TESTS
        .iter()
        .chain(MACOS_ONLY_TESTS.iter())
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    expected.sort();
    let mut unique = expected.clone();
    unique.dedup();
    assert_eq!(
        unique, expected,
        "a test is listed as both Linux and macOS-only"
    );
    assert_eq!(
        found, expected,
        "classify every tests/*.rs file in LINUX_TESTS or MACOS_ONLY_TESTS (and docs/BUILD.md)"
    );
    let source = |name: &str| {
        fs::read_to_string(root().join("tests").join(format!("{name}.rs")))
            .unwrap_or_else(|e| panic!("cannot read tests/{name}.rs: {e}"))
    };
    // This file mentions the markers itself, so it is not scanned.
    for name in LINUX_TESTS.into_iter().filter(|n| *n != "ci_workflow") {
        let text = source(name);
        assert!(
            !text.contains("CARGO_BIN_EXE") && !text.contains("#![cfg(target_os"),
            "tests/{name}.rs needs a macOS bin or cfg; move it to MACOS_ONLY_TESTS"
        );
    }
    for name in MACOS_ONLY_TESTS {
        let text = source(name);
        assert!(
            text.contains("CARGO_BIN_EXE") || text.contains("#![cfg(target_os = \"macos\")]"),
            "tests/{name}.rs launches no bin and has no macOS cfg; move it to LINUX_TESTS \
             (CARGO_BIN_EXE only shows that a test launches a bin, not that it needs macOS; \
             say why in docs/BUILD.md)"
        );
    }
}
