//! Documentation must stand alone: relative links resolve, and the README
//! states what Reach is and is not. Offline.
use std::{fs, path::PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn markdown_files() -> Vec<PathBuf> {
    let mut files = vec![root().join("README.md")];
    for entry in fs::read_dir(root().join("docs")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) == Some("md") {
            files.push(path);
        }
    }
    files
}

/// Targets of `](target)` links in `text`.
fn link_targets(text: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("](") {
        rest = &rest[start + 2..];
        if let Some(end) = rest.find(')') {
            targets.push(rest[..end].to_string());
            rest = &rest[end..];
        }
    }
    targets
}

#[test]
fn relative_markdown_links_resolve() {
    let mut broken = Vec::new();
    for file in markdown_files() {
        let text = fs::read_to_string(&file).unwrap();
        for target in link_targets(&text) {
            if target.starts_with("http") || target.starts_with('#') || target.is_empty() {
                continue;
            }
            let path_part = target.split('#').next().unwrap();
            let resolved = file.parent().unwrap().join(path_part);
            if !resolved.exists() {
                broken.push(format!("{}: {target}", file.display()));
            }
        }
    }
    assert!(broken.is_empty(), "broken links:\n{}", broken.join("\n"));
}

#[test]
fn readme_links_the_mcp_wrapper_doc() {
    let readme = fs::read_to_string(root().join("README.md")).unwrap();
    assert!(
        readme.contains("docs/MCP.md"),
        "README.md must link docs/MCP.md"
    );
    assert!(
        !readme.contains("MCP 包裝尚未提供"),
        "README.md still says the MCP wrapper is missing"
    );
}

#[test]
fn mcp_doc_states_the_evaluation() {
    let doc = fs::read_to_string(root().join("docs/MCP.md")).expect("docs/MCP.md");
    for needle in [
        "2025-06-18",
        "stdio",
        "--cu-agent",
        "--helper",
        "ax_helper",
        "任意絕對路徑",
        "<REACH_DIR>",
        "mcpServers",
        "needs_agent",
        "action_allowed",
        "wrapper_error",
        "不支援",
        "65,536",
    ] {
        assert!(doc.contains(needle), "docs/MCP.md lacks {needle:?}");
    }
}

#[test]
fn readme_states_agent_neutrality() {
    let readme = fs::read_to_string(root().join("README.md")).unwrap();
    for needle in [
        "與 agent 無關",
        "docs/AGENT-CLI.md",
        "describe",
        "未驗證",
        "目前只支援 macOS",
        "virtual_input",
    ] {
        assert!(readme.contains(needle), "README.md lacks {needle:?}");
    }
}

/// Plan I-2 (faberline/reach#27): the docs say how input mode starts, its limits, and that
/// input is disabled by default and never offered through MCP.
#[test]
fn docs_state_how_input_mode_starts_and_that_it_is_off_by_default() {
    let read = |p: &str| fs::read_to_string(root().join(p)).unwrap();
    let service = read("docs/AGENT-SERVICE.md");
    for needle in [
        "輸入模式",
        "預設不送任何輸入事件",
        "--enable-input",
        "--allow-bundle",
        "--input-ttl-seconds",
        "--max-input-events",
        "--allow-keycode",
        "org.local.rust-computer-use.target-event-fixture",
        "com.apple.Safari",
        "Chrome 未核准",
        "600",
        "20",
        "MCP 不能啟用輸入",
        "假後端",
        "ReachAgentInputSystem.swift",
        "input_unavailable",
        "`CI`",
        "posted_unverified",
        "delivery_unknown",
        "events_remaining",
        "從未實際執行過",
        "不是安全邊界",
        "防誤用",
        // I-3 adds read-only observations but does not wire them into the gate (I-4).
        "接上之後這一點就不再成立",
        "**沒有**把它們接進閘門",
        "實機未驗證",
        "`used_by_input_gate`",
        "18、23、36、38、44、49",
    ] {
        assert!(
            service.contains(needle),
            "docs/AGENT-SERVICE.md lacks {needle:?}"
        );
    }
    let mcp = read("docs/MCP.md");
    assert!(mcp.contains("`tools/list` 不列出它們") && mcp.contains("`refused`"));
    assert!(read("README.md").contains("**輸入預設停用**"));
    let cli = read("docs/AGENT-CLI.md");
    for needle in ["agent_unavailable", "input_disabled", "沒有 helper 退路"] {
        assert!(cli.contains(needle), "docs/AGENT-CLI.md lacks {needle:?}");
    }
}
