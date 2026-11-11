# 單獨建置通用 Computer Use

本儲存庫是通用 computer-use 程式，目前只支援 macOS，遊戲專用邏輯不在此處。請在儲存庫根目錄執行以下命令。

```sh
cargo build --locked --bin cu-agent
sh build-ax.sh
sh build-capture.sh
sh build-frame-status.sh
sh build-stream-image.sh
sh build-ocr.sh
sh build-cursor-overlay.sh
sh build-agent.sh
sh build-agent-app.sh --adhoc
```

Rust 第一次建置可能需要下載 `Cargo.lock` 指定的套件。已備妥套件時，可以在 `cargo build` 加上 `--offline`。Swift helper 位於 `target/debug`。它們依賴 macOS 系統框架與對應權限；建置成功不代表已取得權限。

```sh
printf '%s\n' '{"id":"check","method":"describe"}' | target/debug/cu-agent
cargo test --locked
```

`describe` 回報支援的單次 JSON 呼叫。詳見 [CLI 文件](AGENT-CLI.md)。ReachAgent 服務（協定、視窗擷取與無障礙快照）的協定見 [ReachAgent 服務協定](AGENT-SERVICE.md)；`cu-agent` 如何把不帶 helper 欄位的擷取與無障礙請求交給 App，見 [CLI 文件](AGENT-CLI.md) 的「經由 ReachAgent App 的路徑」。helper 路徑與輸出路徑必須使用呼叫者目前環境中的絕對路徑。不要沿用其他機器的範例路徑。

目前框架提供唯讀視窗、無障礙、截圖、OCR 與證據；結果需要 Agent 覆核。背景輸入、游標隔離、焦點隔離及圖片到視窗的座標尚未驗證。一般操作維持停用。

2026-11-09 將本目錄複製至獨立的暫存目錄，排除 `target`，以離線 Cargo 執行完整預設測試。測試通過；部分原生記憶體事件測試按設計為 ignored。AX、單張擷取、影格狀態、串流圖片和 OCR helper 可在該副本建置；OCR 對既有圖片的測試在系統環境成功，一般沙箱回報錯誤。這只證明當次建置和有限測試，不代表輸入或跨機器權限已驗證。

部分歷史文件原本連結到來源專案的 `benchmark` 或 `screenshots`。這些證據未隨本庫攜帶，文件已改為純文字說明（「原始證據：來源專案 …」）。需保留來源專案或另行整理證據後，才能重現那些紀錄。來源與匯入紀錄見 [IMPORT-PROVENANCE.md](IMPORT-PROVENANCE.md)。

## CI

CI 只有一個 workflow（`.github/workflows/ci.yml`，名稱 `ci`），依 aw 的 CI 階段標準分成 `lint → test-linux／test-macos → gate`：

- `lint`（`ubuntu-latest`）：只跑 `cargo fmt --all -- --check` 與 `cargo clippy --workspace --all-targets --locked -- -D warnings`。
- `test-linux`（`ubuntu-latest`，顯示名稱 `test: linux (lib, offline)`）：`cargo test --locked --lib`，再加一步只讀檔或純離線的整合測試：`ax_incomplete_docs`、`bin_platform_split`、`calibration_docs`、`ci_workflow`、`docs_links`、`docs_public_paths`、`foreground_input_static`、`import_hygiene`、`platform_boundary`、`provenance_manifest`、`virtual_input`、`window_identity_docs`。
- `test-macos`（`macos-latest`；本儲存庫是公開的，所以免費）：先以 `sh build-capture.sh` 建置擷取 helper 並執行其 `--self-test`（只用合成 PNG，不擷取螢幕，不需權限），再跑 `cargo test --workspace --locked --no-fail-fast`。另外以 `sh build-cursor-overlay.sh` 建置游標 overlay helper 並執行其 `--self-test`（不把視窗放上螢幕，不需權限），見 [游標 overlay](CURSOR-OVERLAY.md)。
- `gate`：`if: always()`，等所有 job；只有 `lint` 成功且其他 job 都成功或略過時才通過。之後若加 ruleset，只需要求 `gate`，名稱不可改。

只在 `test-macos` 跑的整合測試是 `cli`、`cu_agent_cli`、`cu_mcp_cli`、`focus_watch_cli`、`headless_check_cli`、`hidden_appkit_check_cli`、`hidden_window_check_cli` 與 `macos_event_memory`。原因有三種：`cli` 與 `cu_agent_cli` 用 `CARGO_BIN_EXE_*` 啟動 `mac-computer-use` 與 `cu-agent`，這兩個 bin 在非 macOS 只回報不支援，行為與測試預期不同；`cu_mcp_cli` 用 `CARGO_BIN_EXE_*` 啟動 `cu-mcp` 與 `cu-agent`，預期的是 macOS 上 `cu-agent` 的回覆；`focus_watch_cli`、`headless_check_cli`、`hidden_appkit_check_cli`、`hidden_window_check_cli` 是 `#![cfg(unix)]` 的程序測試，用 `CARGO_BIN_EXE_*` 啟動監督用 bin 並以 `/bin/sh` 假 helper 驅動，不是讀檔測試，也尚未在 Linux 驗證，所以留在 `test-macos`；`macos_event_memory` 是 `#![cfg(target_os = "macos")]`，使用 Core Graphics。bin 內部的單元測試也只在 `test-macos` 跑。新增 `tests/*.rs` 時必須歸入 Linux 或 macOS-only 其中一類，並更新 `.github/workflows/ci.yml` 與本節；`tests/ci_workflow.rs` 會檢查。

`lint` 在 Linux 執行 `cargo clippy --workspace --all-targets --locked -- -D warnings`（`--workspace --all-targets` 涵蓋 lib、全部 bin 與測試 profile），所以 bin 在 Linux 通過型別檢查與 clippy，不需另外的 bin clippy 指令。

需要螢幕錄製、輔助使用權限或 window server 的測試標 `#[ignore = "<原因>"]`，不在 CI 執行，只在已授權的本機以 `cargo test --locked -- --ignored` 執行。

需要解碼 PNG 或計算 SHA-256 的 lib 單元測試只在 macOS 編譯（以 `#[cfg(target_os = "macos")]` 標註，不使用 `#[ignore]`），所以 `test-linux` 不會執行它們（`test-macos` 會）。`src/lib.rs` 內與擷取相關的有 `png_sha256_constant_matches_the_native_decoder`、`capture_is_unknown_even_when_geometry_matches`、`title_change_does_not_change_identity`、`all_window_capture_succeeds_with_complete_inventory`、`records_success_without_claiming_image`、`sidecar_records_before_and_after_windows`、`refuses_dimensions_that_do_not_match_the_decoded_png_and_preserves_evidence`；`src/calibration.rs` 內使用 `fitted` 輔助函式（需要 SHA-256 的 evidence id）的測試也一樣。其餘檢查（視窗或程序身分改變、metadata 不符、證據檔紀錄等）使用固定的測試摘要，會在 CI 執行。

需要權限的部分（標 `#[ignore]` 的測試）必須在已授權的本機以 `cargo test --locked -- --ignored` 執行並把輸出貼在 PR；CI 不驗證它們。

## 權限與簽章

macOS 的 TCC 權限（Accessibility、Screen Recording、事件送出）綁在 App 的簽章身分上。直接從 terminal 或 agent 的 shell 執行 CLI 時，權限會被算到上層程序（terminal、agent）。ad-hoc 簽章的 App 每次重新建置都會改變 cdhash，使用者必須重新授權，因此**重新建置會掉授權**。未來需要權限的動作應交給有固定 bundle id（建議 `dev.faberline.reach.*`）與穩定簽章的獨立 helper App。三種權限須分開回報：Accessibility、Screen Recording（`CGPreflightScreenCaptureAccess`）、事件送出（`CGPreflightPostEventAccess`）；缺任一項回報 unknown／denied，不得靜默降級。

ReachAgent 服務（見 [ReachAgent 服務協定](AGENT-SERVICE.md)）就是這個 helper App：bundle id `dev.faberline.reach.agent`、`LSUIElement`（沒有 Dock 圖示）、`ReachAgent-Info.plist` 為三種權限各有用途說明。`sh build-agent-app.sh` 建置、簽章並安裝到 `~/Applications/ReachAgent.app`；`sh build-agent-app.sh --adhoc` 只在暫時目錄建置並做 ad-hoc 簽章、不安裝，CI 使用這個模式。

`cu-agent` 的 `capture`、`capture_all_windows`、`accessibility`、`accessibility_subtree`（不帶 helper 欄位時）現在經由這個 App 取得權限；App 沒有執行時回報 `agent_unavailable`，不退回 terminal 的權限（見 [CLI 文件](AGENT-CLI.md)）。

建立本機憑證與安裝由使用者自己做；腳本與 Reach 絕不建立、匯入或信任憑證，也不改鑰匙圈與權限：

1. 一次性建立憑證：開啟「鑰匙圈存取」，選單「憑證輔助程式」→「建立憑證⋯」，名稱例如 `Reach Local Signing`，身分類型選「自簽根憑證」，憑證類型選「程式碼簽署」。建立後可自己執行 `security find-identity -v -p codesigning` 確認名稱。
2. 安裝：`REACH_SIGN_IDENTITY="Reach Local Signing" sh build-agent-app.sh`。未設定 `REACH_SIGN_IDENTITY` 時腳本會在建置前停止並說明原因，**不會退回 ad-hoc**。
3. 驗證身分穩定：連續兩次建置印出的 designated requirement 必須相同（自簽憑證版本形如 `identifier "dev.faberline.reach.agent" and certificate leaf = H"…"`）。`--adhoc` 版本是 `cdhash`，每次建置可能不同，所以只供 CI 與建置檢查。
4. 授權：到系統設定的隱私權與安全性，由使用者自己把 `~/Applications/ReachAgent.app` 加入「輔助使用」與「螢幕與系統錄音」。腳本不改 TCC。

限制：不含 Developer ID、公證與散布到其他 Mac，也不含登入項目自動啟動。
