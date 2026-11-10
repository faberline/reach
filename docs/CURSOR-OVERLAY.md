# 游標 overlay（只顯示）

這份文件說明游標 overlay helper `mac-cursor-overlay` 與其協定 **`cursor-overlay-v1`**。helper 只負責在畫面上顯示一個標記，不送輸入、不移動真實游標、不搶焦點。

## 建置與自測

helper 以 `sh build-cursor-overlay.sh` 建置，產出 `target/debug/mac-cursor-overlay`。建置腳本會接著執行 `--self-test`。自測只跑純資料檢查與一個從未顯示在螢幕上的 panel 屬性檢查，**自測不會把視窗放上螢幕**，最後一行輸出是 `{"status":"self_test_passed","window_shown":false}`。

## 協定

helper 有兩種模式：

- `--self-test`：離線自測，見上節。
- `--serve`：從 stdin 每行讀一個 JSON 請求（一行最多 4096 bytes），對每個請求在 stdout 回一行 JSON 回覆。stdin 結束或收到 `quit` 時隱藏 overlay 並結束。

協定版本是 **`cursor-overlay-v1`**，每個請求的 `protocol` 欄位都必須是這個字串。

### 請求欄位

| command | 鍵恰好是 |
|---|---|
| `show`、`move` | `protocol`、`id`、`command`、`point` |
| `phase` | `protocol`、`id`、`command`、`phase` |
| `hide`、`query`、`quit` | `protocol`、`id`、`command` |

- `id`：1 到 64 bytes 的字串，每個 byte 在 `0x21` 到 `0x7e`（不含空白）。回覆會原樣帶回。
- `point`：物件，鍵恰好是 `space`、`display_id`、`x`、`y`。
- `phase`：`idle`、`click`、`hold` 或 `drag`。

六個 command 的意義：

- `show`：在指定座標顯示 overlay，phase 變為 `idle`。
- `move`：把已顯示的 overlay 移到新座標，需已顯示。
- `phase`：只改變外觀（`idle`／`click`／`hold`／`drag`），需已顯示。
- `hide`：隱藏 overlay。
- `query`：回傳目前狀態，不改變狀態。
- `quit`：隱藏 overlay 並結束 helper。

### 座標

- `space` 固定是 **`global_display_points_top_left`**：全域顯示點，原點在主螢幕左上角，y 向下。
- `display_id` 是呼叫者宣稱的螢幕；helper 會確認該螢幕存在，且座標落在它的範圍內。
- `x`、`y` 必須是有限的數字，絕對值不可超過 1000000。
- `display_id` 必須是 1 到 4294967295 之間的整數。

### 回覆

每個回覆都是 9 個欄位的物件：`protocol`、`id`、`status`、`reason`、`visible`、`phase`、`point`、`window_binding`、`action_allowed`。

`status` 是 `ok` 時 `reason` 為 `null`；是 `error` 時 `reason` 是下列其中之一：

- `invalid_json`：不是合法 JSON 物件，或 JSON 有重複鍵。
- `invalid_request`：`protocol` 不對、`id` 不合法、缺少或多出欄位。
- `unknown_command`：`command` 不在六個之中。
- `invalid_point`：`point` 不符合格式或範圍。
- `invalid_phase`：`phase` 不是四個合法值之一。
- `not_visible`：`move` 或 `phase` 時 overlay 尚未顯示。
- `unknown_display`：`display_id` 指定的螢幕不存在。
- `point_outside_display`：座標不在該螢幕範圍內。
- `display_changed`：overlay 顯示中而螢幕配置改變時，helper 自動隱藏，並主動送出一行 `id` 為 `null` 的回覆。這是唯一「發生錯誤時狀態不變」的例外。

除了 `display_changed`，發生錯誤時狀態不變。

## 顯示不等於授權

overlay 只是畫面上的標記。回覆固定是 `"window_binding": "unbound"` 與 `"action_allowed": false`，表示這個標記**不授權輸入**，也**未綁定視窗**。overlay 的位置沒有綁定任何視窗、程序或擷取結果，視窗移動後它不會跟著動。`click`／`hold`／`drag` 只改變外觀，不代表有任何事件送出。下游若要做任何動作，必須有自己獨立的授權與驗證，不能只依據 overlay 的狀態。

## 權限

helper 不需要、也不要求 **Accessibility**、**Input Monitoring** 與 **Screen Recording** 這三種權限。它不讀其他視窗、不擷取螢幕、不送事件、不移動真實游標，也不啟用自己：App 以 `accessory` 模式執行，panel 是 non-activating panel，並設定 `ignoresMouseEvents`，滑鼠事件會穿透，panel 不能成為 key 或 main window。

## 限制

- `cu-agent` 的 `overlay_session` 提供逾時與清除，見 [Agent CLI](AGENT-CLI.md)。
- 尚未在實機畫面驗證，這是之後的工作。
- `display_changed` 尚未在實機驗證。
- helper 的 JSON 解析不拒絕重複鍵；Rust 側 `cursor_overlay::parse_request` 會拒絕重複鍵。
