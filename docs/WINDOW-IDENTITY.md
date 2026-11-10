# 視窗身分與擷取證據

本文說明 `capture`（`observe_with_evidence`）如何判定目標視窗、sidecar 證據記了什麼，以及哪些事情**沒有**被證明。所有結果仍是 `decision=unknown`、`action_allowed=false`，不授權任何輸入。

## 身分如何判定

目標由 `pid` 與 `window_id` 組成（`Target`）。擷取只在下列條件全部成立時才算一次完整觀察：

1. 擷取前取得程序身分（`Backend::identity`）：`status` 為 `ok`、`pid` 相符、`process_start_unix_seconds` 為大於 0 的有限值。
2. 擷取前視窗盤點中，該 `Target` 恰好出現一次、幾何有效（座標有限、寬高為正；x、y 可為負）、Core Graphics layer 為 0。零個、多個（重複候選）、無效幾何、layer 缺失或非 0 都停止，且不呼叫擷取器。無效幾何的項目在計算重複候選之前就被濾掉，所以一個有效加一個無效的重複項不會被判為 ambiguous；只有無效項時回報目標視窗目前不可見。
3. 擷取後再盤點一次，`Target`、幾何與 layer 不得改變。
4. 擷取後再取得一次程序身分，必須與擷取前完全相同（PID 重用或程序重啟會不同）。
5. 擷取器回報的 `window_id`、`owner_pid`、`image_path`、視窗框與盤點相符；圖片 SHA-256 與實際解碼的 PNG 尺寸相符。

**視窗標題不是身分**：標題可變、可缺、可重複。前後標題不同不會讓觀察失敗，也不會讓不同視窗變成同一個。舊 PID、舊 `window_id` 單獨也不構成身分。

## 證據欄位

sidecar 是 `<圖片檔名>.json`（例如圖片 `new.png` 的 sidecar 是 `new.png.json`），以 0600 權限先建立（既有檔案不覆寫），之後寫入結果。

- `evidence_version`：目前為 `2`。2 新增 `window_before` 與 `window_after`；其餘既有欄位意義不變。
- `window_before`、`window_after`（成功時）：擷取前後各一份視窗快照，欄位 `pid`、`window_id`、`layer`、`title`（可為 `null`）、`bounds_points`（`x`、`y`、`width`、`height`，Quartz 螢幕點座標）。
- `process_identity`（成功時）：擷取前的程序身分（PID、啟動時間與來源等）。
- `capture`（成功時）：擷取器的 metadata，包含圖片路徑、SHA-256、像素寬高與視窗框。
- 錯誤 sidecar（`status=error`）沒有 `process_identity`、`capture`（欄位不存在，不是 `null`），改有：`error`（原因）、`requested_image_path`、`image_ownership`（固定為 `unverified`）。
- `inventory_scope`、`evidence_file`（sidecar 自己的路徑）：成功與錯誤 sidecar 都有。`inventory_scope` 是盤點範圍，例如 `onscreen_candidates`。
- `status`：`captured` 或 `error`；`decision` 一律為 `unknown`；`action_allowed` 一律為 `false`。

`stream_status` 的 `window_before`／`window_after` 與 sidecar 的形狀不同：欄位是 `pid`、`window_id`、`layer`、`bounds`（`x`、`y`、`width`、`height`），沒有 `title`；不要把兩者混用。

## 失敗與保留的證據

- 輸出路徑不是以 `.png` 結尾的絕對路徑、圖片或 sidecar 已存在時，在呼叫擷取器之前就拒絕，不覆寫，也不會留下新的 sidecar。
- 擷取或檢查失敗時，sidecar 仍會寫入 `status=error`、原因與 `evidence_version`，並把可能存在的圖片標為 `image_ownership=unverified`；圖片不會被刪除。失敗後請查看 sidecar，不要自動重試。
- 若 sidecar 本身寫入失敗，可能留下空檔或部分檔案，不應視為成功。

## 已知限制

- **遮擋**：layer 0 與擷取成功都不證明視窗沒有被其他視窗遮擋，也不證明它在最上層或擁有焦點。
- **畫面新鮮度**：時間戳與 SHA-256 只證明「這份檔案的內容是這個雜湊」。系統時間可被調整，雜湊不證明畫面是最新的。
- **非原子**：程序身分、視窗盤點、擷取、再盤點、再取身分是依序執行的；兩次相同也不能排除中間的變動。
- **螢幕比例**：圖片像素尺寸與視窗點數的比例（例如 Retina 的 2 倍）只是觀察到的數字，不是座標校準，不能拿來換算點擊位置。

座標與 AX 綁定的研究屬於 issue #3，輸入屬於 issue #5。

## 實測程序（唯讀）

先建置 helper：`sh build-capture.sh`（需要 Xcode Command Line Tools）與 `cargo build --locked --bin mac-computer-use`。

1. `target/debug/mac-computer-use windows --pid PID` 列出候選視窗。
2. `target/debug/mac-computer-use capture --pid PID --window WINDOW_ID --output /absolute/new.png` 擷取，並檢查旁邊的 `.json`。

規則：

- 只對**自己啟動的測試 fixture App**與**使用者已在執行的第二個 App**做唯讀擷取，且圖片只存在暫存目錄、不入庫。
- 只在螢幕錄製權限**已授與**時進行。helper 以 `CGPreflightScreenCaptureAccess` 先檢查，**不會**要求權限或跳出授權視窗；沒有授權時以結束碼 77 結束、不產生圖片。不得更動任何系統權限（TCC）。
- 不送任何輸入，不啟動或控制其他 App。
- 權限未授與時不要嘗試取得；記錄「未執行」並另開 issue 追蹤。

### 實測紀錄

**2026-11-09（macOS，從 agent 的 shell 執行）**：未能實測。直接呼叫擷取器 `target/debug/mac-window-capture 999999999 1 <暫存路徑>.png`（不存在的視窗，僅用來探測權限）回報 `capture failed: screen recording permission is not granted`，結束碼 77，沒有產生圖片，也沒有跳出授權視窗。因此「兩個不同 App 的實機擷取」尚未完成，由 issue #16「多 App 視窗擷取實測（#2 的實機驗證）」追蹤。上述各項負面案例（重複候選、不可見、無效幾何、程序重啟、視窗改變、metadata 不符等）由離線測試涵蓋，見 `src/lib.rs` 的 `mod tests`。
