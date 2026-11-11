# 前景輸入的離線模型（計畫 I-1）

**這是離線模型，本身不投遞。** 每個回應的 `action_allowed` 仍為 `false`。I-2（issue #27）之後，`cu-agent` 只用這個模型檢查前景 `action` 請求的格式，再轉給 ReachAgent；閘門、重放防護與收據在 ReachAgent 內以 Swift 移植（`ReachAgentInput.swift`），順序、閘門名稱與原因字串都相同。`cu-mcp` 不使用這個模型，也不提供輸入。輸入預設停用，啟動方式見 [AGENT-SERVICE.md](AGENT-SERVICE.md) 的「輸入模式」。

模型的來源是已核准的計畫 `docs/plans/input-injection-plan.md`（第 3、4.3、4.4 節與 I-1）。程式在 `src/foreground_input.rs`，`input_guard` 加了前景變體 `review_foreground`。整個模型只有純 Rust 資料與 trait，不呼叫任何事件投遞 API、不用 FFI、不啟動程序。唯一的 `EventPoster` 實作是測試用的假後端。

## 範圍

- 三種事件：`pointer_move`、`click`（只接受左鍵、視窗內點座標）、`key_tap`（只接受計畫 1.3 的六個鍵碼 18、23、36、38、44、49；修飾鍵 54 至 63、功能鍵與系統鍵一律拒絕）。沒有拖曳、滾輪、修飾鍵、文字輸入、啟用 App。
- 請求欄位嚴格比對，任何未知欄位都拒絕（`refused`，閘門 `schema`）。`delivery` 只接受 `foreground`；`background` 回報 `unsupported`（閘門 `delivery`）。
- 允許清單只有 `org.local.rust-computer-use.target-event-fixture` 與 `com.apple.Safari`；Chrome 未核准。`Arming::new` 拒絕清單外的 bundle id（沒有萬用字元）、超過 10 分鐘的到期、零事件數或超過 `MAX_ARM_EVENTS`（20）的事件數、`APPROVED_KEYCODES` 以外的鍵碼。計畫 Q6 只要求每次啟用設事件數上限、沒有給數字；20 是為 I-4 至 I-6 的少量單擊與按鍵實驗選的保守上限，實機核准時可再調。
- 四種狀態與結束碼：`unsupported`（3）、`refused`（1）、`posted_unverified`（3）、`delivery_unknown`（1）。

## 閘門

預設（`InputGate::default()`）是停用：任何請求都回 `unsupported`（G1 `input_disabled`）。啟用後依序檢查：

| 閘門 | 拒絕原因 |
|---|---|
| G1 | `arming_expired`、`event_limit_reached` |
| G2 | `bundle_not_allowed`、`keycode_not_allowed` |
| G3 | `bundle_mismatch`、`process_start_mismatch`、`target_mismatch`、`identity_mismatch`、`invalid_observation` |
| G5 | `observation_mismatch`、`observation_expired`、`observation_already_used` |
| G6 | `point_outside_window`、`point_in_title_bar`（只對有座標的事件） |
| G7 | `point_occluded`（只對有座標的事件） |
| G8 | `target_not_frontmost`、`focus_changed` |
| G9 | `hid_not_idle`、`modifier_held`、`mouse_button_down`、`secure_input_enabled`、`input_source_unexpected` |
| G12 | `attempt_not_recorded`（寫不進 `attempt` 就不投遞） |
| G10 | `press_not_sent`（狀態 `refused`；見下方「收據與不重試」） |
| G13 | `halted_after_delivery_unknown`、`request_already_executed`、`request_id_reused` |

G3、G7 至 G9 所需的事實由後端提供（`LiveFacts`，包含 `current_bundle_id`、`target_frontmost`、`focus_unchanged`、`hid_idle`、`modifiers_released`、`mouse_buttons_released`、`secure_input_off`、`input_source_ok`、`point_unoccluded`）；模型只消費它們，**無法自己確認**。I-2 必須確保這些值來自 ReachAgent 自己的取樣（程序身分、視窗清單、焦點、HID 與安全輸入狀態），絕不能由請求帶入或由呼叫端填寫；否則 G3、G7 至 G9 等於沒有檢查。

## 重放防護

同一個請求不能投遞兩次，舊的觀察也不能替下一個事件背書：

- `InputGate` 記住每個已寫入 `attempt` 的請求。同一 `id` 且內容完全相同再提交 → `refused`，G13 `request_already_executed`；同一 `id` 但內容不同 → G13 `request_id_reused`。
- 每次寫入 `attempt` 後，該請求用的 `image_sha256` 即失效；下一個事件必須帶新的觀察，沿用舊的 → G5 `observation_already_used`。
- 只有 `attempt` 寫成功才算「用掉」。`attempt_not_recorded`（什麼都沒投遞）不用掉請求與觀察。投遞失敗或 `delivery_unknown` 則照樣用掉。
- 這些拒絕不消耗事件數，也不投遞、不寫收據。重新啟用（新的 `InputGate`）才會清空記錄。

## 收據與不重試

投遞前先寫 `attempt`（`reach-input-attempt-v1`），成功才計數並投遞；投遞後寫 `result`（`reach-input-result-v1`）。`EventPoster` 的每個呼叫失敗時要分辨兩種錯誤：`PostError::NotSent`（後端確定作業系統沒有收到事件）與 `PostError::MaybeSent`（其他一切情況）。按下可能已送出而後失敗時，嘗試放開一次（`release_attempted`／`release_confirmed`），這是清理而不是重試。按下確定沒送出（`NotSent`）時不呼叫放開，因為沒有對應按下的放開本身就是多餘事件；結果是 `refused`，G10 `press_not_sent`，事件數已用掉，閘門不停機。這個取捨與計畫 G10 一致（G10 要求的是「按下後」失敗才放開），後端只在確定時才能回 `NotSent`，不確定一律回 `MaybeSent`。事後檢查失敗、`result` 寫不進去，或放開失敗，都是 `delivery_unknown`，且之後這個閘門拒絕所有請求（G13），直到以新的啟用取代。`recover_status` 把「有 `attempt` 卻沒有對得上的 `result`」讀成 `delivery_unknown`。

## I-3 閘門強化（計畫審查 R1、R2、N1、R4）

- **R1 標題列（G6）**：座標是相對於整個視窗外框（含標題列），離線無法知道內容區的確切範圍，也無法分辨綠色縮放鈕與其他按鈕。因此從視窗頂端起固定 52 pt（`TITLE_BAR_BAND_POINTS`／`titleBarBandPoints`；涵蓋一般 28 pt 標題列與含工具列的統一標題列）一律拒絕，原因 `point_in_title_bar`，在「視窗外」檢查之後、G7 之前。52 pt 以下若還有分頁列或更高的工具列，這裡偵測不到。**綠色按鈕（以及精確的內容區）要等 I-4 在實機上取得內容區與按鈕位置後再處理**；在那之前標題列整段都不能點。Rust 與 Swift 用同一個值，靜態測試比對兩邊。
- **R2 按住的修飾鍵與滑鼠鍵（G9）**：新增兩個事實 `modifiers_released`、`mouse_buttons_released`，在 `hid_not_idle` 之後檢查，原因 `modifier_held`、`mouse_button_down`。修飾鍵遮罩為 Caps Lock、Shift、Control、Option、Command、Fn 六個旗標位元（`HELD_MODIFIER_FLAGS_MASK`／`heldModifierFlagsMask`，以原始數值寫成，兩邊由靜態測試比對）；數字鍵盤位元不算。`ReachAgentInputSystem.swift` 加了唯讀取樣函式 `sampleHeldInput()`（`CGEventSource.flagsState(.hidSystemState)` 與左、右、中三個滑鼠鍵的 `buttonState`），**但沒有接進系統後端的 `liveFacts`**：兩個事實在系統後端仍是 fail-closed 的 `false`，接線與實機驗證屬於 I-4。第 4 個以上的滑鼠鍵不在取樣範圍。
- **N1 呼叫點檢查**：`authorize` 與 `makeInputController` 改用 token 比對（等同 `\bname\s*\(`，可跨行、允許空白），系統檔案中 `authorize` 的定義行也列入已知清單。系統檔案以外的 `SystemInputLaunch` 每次出現都必須在已知清單內，且禁止 `extension SystemInputLaunch`。測試資料包含三種繞過：extension 內呼叫 `authorize(enableInput: true, testMode: false)`、型別與 `.authorize(` 分兩行、`makeInputController (` 中間有空白。
- **R4 全域禁用**：ReachAgent 編譯的每個 Swift 檔（含系統後端）都不得出現 `CGWarpMouseCursorPosition`、`CGDisplayMoveCursorToPoint`、`CGEvent.init`、`osascript`、`NSTask`、`Process(`、`.activate(`、`activateIgnoringOtherApps`、`NSAppleScript`；比對前先去掉空白。

R3、R5 不在這次範圍。

## 尚未做的事

I-2（issue #27）加入了 ReachAgent 輸入後端骨架、`--enable-input`、`describe` 的 `input` 區塊，以及 MCP 不暴露輸入的過濾，見 [AGENT-SERVICE.md](AGENT-SERVICE.md)。系統後端的即時取樣（焦點、HID 閒置、安全輸入、輸入法、遮擋）屬於 I-3，目前一律視為不安全。實機驗證屬於 I-4 之後。
