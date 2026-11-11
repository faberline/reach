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
| G6 | `point_outside_window` |
| G7 | `point_occluded`（只對有座標的事件） |
| G8 | `target_not_frontmost`、`focus_changed` |
| G9 | `hid_not_idle`、`secure_input_enabled`、`input_source_unexpected` |
| G12 | `attempt_not_recorded`（寫不進 `attempt` 就不投遞） |
| G10 | `press_not_sent`（狀態 `refused`；見下方「收據與不重試」） |
| G13 | `halted_after_delivery_unknown`、`request_already_executed`、`request_id_reused` |

G3、G7 至 G9 所需的事實由後端提供（`LiveFacts`，包含 `current_bundle_id`、`target_frontmost`、`focus_unchanged`、`hid_idle`、`secure_input_off`、`input_source_ok`、`point_unoccluded`）；模型只消費它們，**無法自己確認**。I-2 必須確保這些值來自 ReachAgent 自己的取樣（程序身分、視窗清單、焦點、HID 與安全輸入狀態），絕不能由請求帶入或由呼叫端填寫；否則 G3、G7 至 G9 等於沒有檢查。

## 重放防護

同一個請求不能投遞兩次，舊的觀察也不能替下一個事件背書：

- `InputGate` 記住每個已寫入 `attempt` 的請求。同一 `id` 且內容完全相同再提交 → `refused`，G13 `request_already_executed`；同一 `id` 但內容不同 → G13 `request_id_reused`。
- 每次寫入 `attempt` 後，該請求用的 `image_sha256` 即失效；下一個事件必須帶新的觀察，沿用舊的 → G5 `observation_already_used`。
- 只有 `attempt` 寫成功才算「用掉」。`attempt_not_recorded`（什麼都沒投遞）不用掉請求與觀察。投遞失敗或 `delivery_unknown` 則照樣用掉。
- 這些拒絕不消耗事件數，也不投遞、不寫收據。重新啟用（新的 `InputGate`）才會清空記錄。

## 收據與不重試

投遞前先寫 `attempt`（`reach-input-attempt-v1`），成功才計數並投遞；投遞後寫 `result`（`reach-input-result-v1`）。`EventPoster` 的每個呼叫失敗時要分辨兩種錯誤：`PostError::NotSent`（後端確定作業系統沒有收到事件）與 `PostError::MaybeSent`（其他一切情況）。按下可能已送出而後失敗時，嘗試放開一次（`release_attempted`／`release_confirmed`），這是清理而不是重試。按下確定沒送出（`NotSent`）時不呼叫放開，因為沒有對應按下的放開本身就是多餘事件；結果是 `refused`，G10 `press_not_sent`，事件數已用掉，閘門不停機。這個取捨與計畫 G10 一致（G10 要求的是「按下後」失敗才放開），後端只在確定時才能回 `NotSent`，不確定一律回 `MaybeSent`。事後檢查失敗、`result` 寫不進去，或放開失敗，都是 `delivery_unknown`，且之後這個閘門拒絕所有請求（G13），直到以新的啟用取代。`recover_status` 把「有 `attempt` 卻沒有對得上的 `result`」讀成 `delivery_unknown`。

## 尚未做的事

I-2（issue #27）加入了 ReachAgent 輸入後端骨架、`--enable-input`、`describe` 的 `input` 區塊，以及 MCP 不暴露輸入的過濾，見 [AGENT-SERVICE.md](AGENT-SERVICE.md)。系統後端的即時取樣（焦點、HID 閒置、安全輸入、輸入法、遮擋）屬於 I-3，目前一律視為不安全。實機驗證屬於 I-4 之後。
