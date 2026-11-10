# Reach

通用的 computer-use 應用，以 Rust 撰寫。**目前只支援 macOS**（0.1.0，crate `mac-computer-use`）。

Reach 與 agent 無關：它不依賴 Claude、Codex 或任何 agent 的 API、設定檔或 plugin 格式。任何能啟動程序並讀寫 JSON 的 agent 或程式，都用同一個 CLI 呼叫它。Reach 負責提供可檢查的觀察（視窗、無障礙資料、截圖、OCR、證據）與未來經驗證的操作；判斷與決策由呼叫端負責。遊戲或特定 App 的邏輯不屬於 Reach。

## 與 surface 的分工

Reach 管工具：視窗、無障礙樹、截圖、OCR、證據，以及之後經驗證的輸入。[surface](https://github.com/faberline/surface) 是 UI 框架，要驗證什麼由它決定；它的真機驗證把 `cu-agent` 當測試工具來呼叫（faberline/surface#99）。Reach 不含任何 surface 專用邏輯。輸入目前仍一律停用，見下方「現況」。

## 任何 agent 怎麼呼叫

`cu-agent` 每次讀一個 JSON 請求、輸出一個 JSON 回應（stdin → stdout）。先問它支援什麼：

```sh
cargo build --locked --bin cu-agent
printf '%s\n' '{"id":"check","method":"describe"}' | target/debug/cu-agent
```

`describe` 回報目前支援的方法、欄位與結束碼。完整說明見 [docs/AGENT-CLI.md](docs/AGENT-CLI.md)。helper 路徑與輸出路徑必須使用呼叫者環境中的絕對路徑。MCP 包裝尚未提供（見 issue #6）。

## 現況

- 唯讀：視窗清單、無障礙樹、單張擷取、影格狀態、串流圖片、OCR，以及附帶程序身分與時間的證據。結果需要 Agent 覆核，不授權任何操作。
- **輸入一律停用**：`action` 請求回報不支援，不送出任何事件。背景輸入、實體游標隔離、焦點隔離與圖片到視窗的座標都**未驗證**。匯入既有程式不代表這些已被驗證。
- 離線虛擬輸入：`virtual_input::VirtualSession` 與 `cu-agent` 的 `simulate_input` 是離線的虛擬游標狀態機（虛擬位置、按住的按鍵、事件順序），不送真實事件，是背景輸入（issue #2）的起點，見 [docs/VIRTUAL-INPUT.md](docs/VIRTUAL-INPUT.md)。
- 完整能力與限制清單見 [docs/CAPABILITIES.md](docs/CAPABILITIES.md)。

## 平台

目前只支援 macOS，不包含其他平台的空殼。邊界如下：

- 平台相關（macOS）：`src/platform/macos/`（`platform::macos`）、根目錄的 `*.swift` helper 與 `build-*.sh`（視窗、截圖、無障礙、輸入、權限）。
- 預期與平台無關：請求／回應合約、目標身分、證據與錯誤，以及 `cu-agent` CLI（`src/strict_json.rs`、`src/simulate.rs`、`src/input_guard.rs`、`src/virtual_input.rs`、`src/capture_geometry.rs`、`src/ax_cg.rs`、`src/joint_observation.rs`、`src/display_inventory.rs`、`src/bounded_process.rs`）。
- 平台層已抽出：`Backend` trait 在 crate 根，macOS adapter 在 `src/platform/macos/`。lib 與全部 bin 都已在 Linux target 做 `cargo check` 與 clippy；`mac-computer-use` 與 `cu-agent` 在非 macOS 執行時只回報不支援。細節見 [docs/PLATFORM.md](docs/PLATFORM.md)。

## 建置需求

macOS、Rust（stable）、Xcode Command Line Tools（`swiftc`）。

```sh
cargo build --locked --bin cu-agent
sh build-ax.sh
sh build-capture.sh
sh build-frame-status.sh
sh build-stream-image.sh
sh build-ocr.sh
cargo test --locked
```

Swift helper 輸出在 `target/debug/`。建置成功不代表已取得權限。詳見 [docs/BUILD.md](docs/BUILD.md)。CI 範圍見該文件的 CI 一節。

## 權限

螢幕錄製與輔助使用等權限由 macOS 授予給**App 的簽章身分**，不是授予給 Reach 原始碼。從 terminal 或 agent 的 shell 直接執行時，權限會被算到上層程序；ad-hoc 簽章的 App 每次重新建置都會掉授權。Reach 不會變更任何系統權限。`cu-agent` 的 `capture`、`capture_all_windows`、`accessibility`、`accessibility_subtree`（不帶 helper 欄位時）會交給 ReachAgent App；App 沒有執行時回報 `agent_unavailable`，缺少權限時回報 `permission_denied`，不靜默降級（見 [docs/AGENT-CLI.md](docs/AGENT-CLI.md)；真機行為尚未驗證）。設計備註見 [docs/BUILD.md](docs/BUILD.md)。

## 來源與歷史證據

程式與文件匯入自一個非 git 的既有目錄，來源、複製時間與每個檔案的 sha256 見 [docs/IMPORT-PROVENANCE.md](docs/IMPORT-PROVENANCE.md)。原始歷史證據（截圖與基準紀錄）位於來源專案，**未隨本庫攜帶**；文件中的相關連結已改為純文字說明。

## 文件索引

- [docs/AGENT-CLI.md](docs/AGENT-CLI.md)：JSON CLI
- [docs/CAPABILITIES.md](docs/CAPABILITIES.md)：現有能力與限制
- [docs/BUILD.md](docs/BUILD.md)：獨立建置、權限與簽章
- [docs/WINDOW-IDENTITY.md](docs/WINDOW-IDENTITY.md)：視窗身分、擷取證據與限制
- [docs/CALIBRATION.md](docs/CALIBRATION.md)：座標校準擬合與失效條件
- [docs/PLATFORM.md](docs/PLATFORM.md)：平台邊界與新增平台的步驟
- [docs/VIRTUAL-INPUT.md](docs/VIRTUAL-INPUT.md)：離線虛擬輸入
- [docs/BACKGROUND-INPUT.md](docs/BACKGROUND-INPUT.md)：背景輸入評估（研究，不是驗證）
- [docs/ISOLATED-SESSION.md](docs/ISOLATED-SESSION.md)、[docs/ISOLATION-OBSERVATION.md](docs/ISOLATION-OBSERVATION.md)、[docs/JOINT-OBSERVATION.md](docs/JOINT-OBSERVATION.md)、[docs/OCR.md](docs/OCR.md)：各項觀察與研究
