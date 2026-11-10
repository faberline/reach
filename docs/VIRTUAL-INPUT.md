# Agent 虛擬輸入：離線原型

目標是讓 Agent 透過 CLI 或 MCP 指定視窗，逐步送出移動、按下、放開及鍵盤請求。這份原型只建立資料與記憶體中的事件，不會呼叫 macOS 投遞介面，也不能操作 App。

## 現有 Rust 介面

`virtual_input::VirtualSession` 接受同一個視窗的序號請求。每次都呼叫既有的 `input_guard::review`：檢查背景目標、畫面參照、時限、程序啟動時間、視窗資料和座標範圍。請求也須依序號送出，且觀察序號與時間不可倒退。同一觀察序號不能換成其他內容。這些資料都是呼叫端陳述，程式沒有驗證來源。

視窗矩形端點及視窗原點加相對座標必須是有限值。這只是數值檢查，不保證浮點精度、顯示器比例或坐標對應正確。

支援虛擬移動、左／右／中鍵按下和放開，以及 macOS 虛擬鍵碼按下和放開。它追蹤虛擬位置、按住的滑鼠鍵和鍵盤鍵碼，並在計畫中列出動作前後的狀態。拖曳時的移動須由未來後端根據按住的按鈕轉為對應拖曳事件。輸入順序不合理、畫面過期、目標不符或超界時，此 session 終止。未放開全部虛擬按鍵時不能正常結束。

所有 `PlannedEvent` 及 session 的 `action_allowed()` 都是 `false`。它們不是送達收據，也不能直接當成可執行事件。真實事件送出後，畫面可能改變；未來呼叫端仍須取得新畫面並由 Agent 核對。session 狀態只在記憶體中；程序重啟不能恢復或猜測真實按鍵狀態。它目前沒有接 CLI、MCP、真實鍵盤或滑鼠後端。

滑鼠或按鍵的真實 Down 送出後，程式崩潰、逾時或資料不一致時，目標可能仍認為按鍵被按住。現有 `finish()` 只檢查模擬資料；它不會發送 Up，也不能作為真實按鍵釋放或清理的證據。接入真實後端前，必須設計並驗證有限時、綁定目標的釋放與失敗處理；結果不明時保留證據並停止，不能假稱已釋放。

## 記憶體事件測試

`tests/macos_event_memory.rs` 以 private Core Graphics source 建立合成事件，讀回類型、鍵碼及合成位置，再釋放物件。沒有投遞事件、建立視窗或事件 tap。座標是測試用的全域座標；這不是視窗座標校準。

普通沙箱的 `CGEventSourceCreate` 曾回傳空值。測試標為 ignored，需要有視窗服務存取權的環境明確執行：

```sh
CARGO_HOME=/private/tmp/sw-cargo-cache cargo test --locked --offline --test macos_event_memory -- --ignored --nocapture
```

2026-11-08 在授權程序中，滑鼠移動、左／右／中鍵按下、拖曳及放開，和一般鍵盤按下／放開都能建立及讀回。Shift 按下和放開的類型均為 `flagsChanged`。只建立 Shift 事件後，下一個鍵盤事件沒有自動帶 Shift 旗標。後端需要處理修飾鍵與鍵盤配置；這次結果不證明它會正確送達，也不證明游標或焦點隔離。

## 下一步的實際後端

Apple 提供 [`CGEventPostToPid`](https://developer.apple.com/documentation/coregraphics/cgevent/posttopid%28_%3A%29) 作為可研究的 PID 定向路徑。對需要窗口編號的 App，還需研究以 `NSEvent` 建立帶 AppKit window number 的事件，再轉為 `CGEvent`。AppKit window number 與 CG window ID 的關聯必須逐次核對，不能假設相同。`CGEventCreateMouseEvent` 接受全域座標，而本介面是視窗相對點數；未校準前不可換算或發送。

[Electron 上游測試](https://github.com/electron/electron/pull/54493) 在特定 Electron 視窗看到背景點擊，但也指出背景移動可能被丟棄。這不能證明拖曳、鍵盤或其他 App 可用。[trycua/cua-driver](https://github.com/trycua/cua/blob/cua-driver-rs-v0.41.0/libs/cua-driver/rust/crates/platform-macos/src/tools/click.rs) 的部分背景點擊路徑可能使目標啟動後再恢復使用者焦點；這不符合本專案目前的不切換焦點條件。它也不是本工作階段的 Computer Use 工具。

真正啟用之前，需在受控測試 App 驗證目標收到的每種事件，並獨立監測實體游標、前景、焦點、視窗順序及使用者輸入。要覆蓋按鍵、拖曳、修飾鍵、失焦、超時、程序中斷和按住時出錯。事件送出結果不明時不得自動重試；必須保存證據交 Agent。取樣只能證明觀察到的狀態，不能保證兩次取樣之間沒有瞬間變化。即使測試 App 通過，其他 App 與遊戲仍需各自驗證。依現有規則，驗證安全前不得執行實機輸入測試或啟用後端。

## 原生事件的離線預覽

`native_event_preview::preview` 把一筆離線計畫轉成 Core Graphics 的記憶體事件，讀回類型、測試座標或鍵碼，再立即釋放。呼叫端提供的全域座標只可當測試資料；程式沒有把視窗座標校準成螢幕座標。事件指標不會交給呼叫端，也沒有投遞功能。所有預覽仍回報 `action_allowed=false`。

移動、滑鼠按下與放開、拖曳、一般鍵碼及左右 Shift、Control、Option、Command 有離線對應。程式依虛擬按鍵狀態設定聚合修飾鍵旗標；同一修飾鍵的左右鍵若仍有一側按住，旗標會保留。Caps Lock 和 Fn 仍拒絕。此映射只驗證建立的資料，不證明目標收到、系統按鍵狀態正確、按鍵清理成功，或前景與實體游標不受影響；也不檢查呼叫端的計畫是否來自真實畫面。[Apple 旗標說明](https://developer.apple.com/documentation/coregraphics/cgeventflags/maskshift)

2026-11-08 在授權程序中，測試建立並讀回移動、左鍵拖曳及一般鍵盤事件，然後釋放。沒有投遞事件或操作 App。結果不能推廣到其他事件與後端。

## Agent CLI 的模擬流程

`cu-agent` 的 `simulate_input` 接受虛構視窗的 `width`、`height` 和最多 32 個 `events`。每項要有 `kind`、`x`、`y`；種類可用 `move`、`button_down`、`button_up`、`key_down`、`key_up`。滑鼠按鍵事件另需 `button`（`left`、`right`、`middle`）；鍵盤事件另需 macOS `keycode`。滑鼠按下可直接帶入目標位置，不必先送出移動。放開時的位置必須和最後一次虛擬位置相同；所有按住的鍵必須放開。欄位錯誤、超界、順序錯誤或未放開都拒絕整次請求。

```json
{"id":"rehearsal","method":"simulate_input","width":100,"height":80,"events":[{"kind":"move","x":10,"y":10},{"kind":"button_down","button":"left","x":10,"y":10},{"kind":"button_up","button":"left","x":10,"y":10}]}
```

結果是 `unknown`、狀態碼 3、`action_allowed=false`。此流程使用假身分、假畫面與假時間，不讀取真實視窗，也不建立或投遞 macOS 事件。成功不證明座標正確或輸入安全。它不是前述原生事件預覽，也未提供真實輸入介面。

`preview_native_input` 使用與 `simulate_input` 相同的虛構欄位。它先驗證完整序列，接著以虛構座標建立並讀回記憶體事件，然後立即釋放。結果的 `native_type` 和 `native_modifier_flags` 是記憶體中的資料，不是送達收據。結果仍是 `unknown`、狀態碼 3，且 `action_allowed=false`。此方法不接收 PID、視窗或真實畫面，不投遞事件，也沒有證明游標或焦點隔離。一般沙箱可能無法建立 private event source，這時回報錯誤。

## 背景點擊不要求先移動

2026-11-09 核對 [Electron 的背景點擊測試](https://github.com/electron/electron/pull/54493)：該專案的特定測試環境中，背景點擊可送達，但移動事件可能被丟棄。這不是所有 App 或本機的驗證。

離線規劃的 `button_down` 現在可直接帶入點擊座標，並更新虛擬位置。它不產生或投遞移動事件。`button_up` 仍須在最後一次虛擬位置，拖曳必須先以虛擬移動更新位置。所有結果仍是 `action_allowed=false`。這不證明送達、焦點或游標隔離。

`preview_tagged_input` 在上述預覽上增加 `synthetic_window_id`，必須是大於零的 32 位元整數。它只接受指標移動和滑鼠按鍵事件。完整序列通過檢查後，才建立記憶體事件。結果的 `synthetic_window_fields` 顯示讀回的欄位 91 和 92。

```json
{"id":"tagged","method":"preview_tagged_input","width":100,"height":80,"synthetic_window_id":1193046,"events":[{"kind":"button_down","button":"left","x":10,"y":10},{"kind":"button_up","button":"left","x":10,"y":10}]}
```

這個編號只是測試資料。此預覽沒有驗證真實視窗對應、事件送達、游標或焦點隔離。結果仍為 `unknown`，`action_allowed=false`。不可用它啟動背景操作。
