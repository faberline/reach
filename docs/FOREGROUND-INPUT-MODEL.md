# 前景輸入的離線模型（計畫 I-1）

**這是離線模型，尚無真實投遞。** `action` 在 `cu-agent` 仍一律回報 `unsupported`，每個回應的 `action_allowed` 仍為 `false`；`cu-agent` 與 `cu-mcp` 沒有使用這個模型，`describe` 的輸出沒有變。輸入仍停用，本文件不改變 README、CAPABILITIES、AGENT-CLI、BACKGROUND-INPUT 的任何敘述。

模型的來源是已核准的計畫 `docs/plans/input-injection-plan.md`（第 3、4.3、4.4 節與 I-1）。程式在 `src/foreground_input.rs`，`input_guard` 加了前景變體 `review_foreground`。整個模型只有純 Rust 資料與 trait，不呼叫任何事件投遞 API、不用 FFI、不啟動程序。唯一的 `EventPoster` 實作是測試用的假後端。

## 範圍

- 三種事件：`pointer_move`、`click`（只接受左鍵、視窗內點座標）、`key_tap`（鍵碼 0 至 127、無修飾鍵）。沒有拖曳、滾輪、修飾鍵、文字輸入、啟用 App。
- 請求欄位嚴格比對，任何未知欄位都拒絕（`refused`，閘門 `schema`）。`delivery` 只接受 `foreground`；`background` 回報 `unsupported`（閘門 `delivery`）。
- 允許清單只有 `org.local.rust-computer-use.target-event-fixture` 與 `com.apple.Safari`；Chrome 未核准。`Arming::new` 拒絕清單外的 bundle id（沒有萬用字元）、超過 10 分鐘的到期、零事件數、大於 127 的鍵碼。
- 四種狀態與結束碼：`unsupported`（3）、`refused`（1）、`posted_unverified`（3）、`delivery_unknown`（1）。

## 閘門

預設（`InputGate::default()`）是停用：任何請求都回 `unsupported`（G1 `input_disabled`）。啟用後依序檢查：

| 閘門 | 拒絕原因 |
|---|---|
| G1 | `arming_expired`、`event_limit_reached` |
| G2 | `bundle_not_allowed`、`keycode_not_allowed` |
| G3 | `bundle_mismatch`、`process_start_mismatch`、`target_mismatch`、`identity_mismatch`、`invalid_observation` |
| G5 | `observation_mismatch`、`observation_expired` |
| G6 | `point_outside_window` |
| G7 | `point_occluded`（只對有座標的事件） |
| G8 | `target_not_frontmost`、`focus_changed` |
| G9 | `hid_not_idle`、`secure_input_enabled`、`input_source_unexpected` |
| G12 | `attempt_not_recorded`（寫不進 `attempt` 就不投遞） |
| G13 | `halted_after_delivery_unknown` |

G3、G7 至 G9 所需的事實由後端提供（`LiveFacts`）；模型只消費它們，無法自己確認。

## 收據與不重試

投遞前先寫 `attempt`（`reach-input-attempt-v1`），成功才計數並投遞；投遞後寫 `result`（`reach-input-result-v1`）。按下後任何失敗都嘗試放開一次（`release_attempted`／`release_confirmed`），這是清理而不是重試。事後檢查失敗、`result` 寫不進去，或放開失敗，都是 `delivery_unknown`，且之後這個閘門拒絕所有請求（G13），直到以新的啟用取代。`recover_status` 把「有 `attempt` 卻沒有對得上的 `result`」讀成 `delivery_unknown`。

## 尚未做的事

ReachAgent 輸入後端、`--enable-input`、`describe` 的 `input` 區塊、MCP 不暴露輸入的過濾與測試，都屬於計畫的 I-2（issue #27）。實機驗證屬於 I-4 之後。
