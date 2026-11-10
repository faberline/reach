# 平台邊界

Reach **目前只支援 macOS**。這份文件說明平台相關程式在哪裡、什麼已驗證、什麼沒有。

## 邊界在哪裡

- `Backend` trait 位於 crate 根（`src/lib.rs`）。它是平台無關的合約：視窗清單、精確視窗、截圖、無障礙樹。`observe`、`observe_with_evidence`、證據與錯誤格式都只依賴它。
- 唯一的平台邊界是 `src/platform/`。`src/platform/mod.rs` 只以 `#[cfg(target_os = "macos")]` 宣告 `pub mod macos;`。
- macOS adapter（模組路徑 `platform::macos`）：
  - `backend`：`MacBackend`（實作 `Backend`）、`recognize_image`、`recognize_text`。
  - `windows`：Core Graphics 視窗清單。
  - `codec`：ImageIO 的 PNG 驗證與 CommonCrypto 的 SHA-256。
  - `capture_lock`：截圖互斥鎖。
  - `stream_image`、`stream_status`：影像串流與狀態工具。
- `lib.rs` 以 `pub use` 保留既有公開路徑（`mac_computer_use::MacBackend`、`recognize_image`、`recognize_text`、`stream_image`、`stream_status`），所以 `cu-agent` 與 `mac-computer-use` 的 macOS 路徑不需修改。
- 根目錄的 Swift helper（`*.swift`）與 `build-*.sh` 仍屬 macOS 部分，不在 `src/platform/` 內。

## 尚未搬進 platform 的部分

- `src/display_inventory.rs` 與 `src/native_event_preview.rs` 仍內嵌 `#[cfg(target_os = "macos")]` 區塊，之後再搬進 `platform::macos`。
- `src/bounded_process.rs` 使用 unix 的 libc 綁定，是 unix 而非 macOS 專屬。
- 在非 macOS 上，這些內嵌區塊只讓 lib 編譯通過；非 macOS 的 `png_dimensions` 回傳 `None`、`sha256_bytes` 回傳錯誤。這不是功能支援。

## 新增平台

1. 建立 `src/platform/<os>/`，為該平台寫真實的實作：`Backend`，以及 `png_dimensions`、`sha256_bytes` 的對應。
2. 在 `src/platform/mod.rs` 加上 `#[cfg(target_os = "<os>")]` 的 `pub mod <os>;`，並在 `lib.rs` 加對應的 re-export。
3. 補測試與文件，並更新 `tests/platform_boundary.rs` 的允許清單。

不提供其他平台的 stub：沒有真實實作與測試的平台目錄不得加入（`tests/platform_boundary.rs` 會檢查）。

## 目前限制與檢查

- `mac-computer-use` 與 `cu-agent` 兩個 bin 已能在 Linux target 編譯。非 macOS 執行時只回報 `unsupported platform: only macOS is supported`（`action_allowed=false`、失敗結束碼），`mac-computer-use capabilities` 與 `action` 仍可用。`cu-agent` 的實作在 `src/bin/cu-agent/app.rs`（macOS 專用），`src/bin/cu-agent.rs` 只是依 cfg 分流的入口。
- lib 與全部 bin 已在 Linux target 做 `cargo check` 與 `cargo clippy -D warnings`。這只證明編譯與 lint，不是已驗證的 Linux 支援。需要該 target 的 std 已安裝（不要為此自動執行 `rustup target add`）：

```sh
cargo check --locked --lib --bins --target aarch64-unknown-linux-musl
cargo clippy --locked --lib --bins --target aarch64-unknown-linux-musl -- -D warnings
cargo clippy --locked --lib --bins --profile test --target aarch64-unknown-linux-musl -- -D warnings
```

若 Homebrew 的 `rustc` 遮蔽了 rustup，請把 rustup stable toolchain 的 `bin` 放在 `PATH` 最前面。
