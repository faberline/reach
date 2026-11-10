# 單次視窗觀察

`cu-agent` 的 `observe_window` 接受 `pid`、`window_id`、新的絕對 PNG 路徑 `image`、`capture_helper` 及 `ax_helper`。它先保留 `.binding.json`，再擷取圖片並保存 `.json`；接著讀取 AX、CG 及顯示器資料。它比較程序身分、擷取視窗的前後 CG 幾何與圖片雜湊。

完整觀察在 CLI 外層回報 `unknown`、狀態碼 3。觀察未完成時，CLI 外層回報 `observation_incomplete`、狀態碼 1；內層仍保留 `unknown`、`incomplete` 和錯誤證據。所有結果均為 `action_allowed=false`。`capture_identity_and_cg_geometry_matched_unverified` 只表示上述資料相符。它不代表 AX 視窗已綁定、畫面未遮擋、座標已校準或輸入安全。這些取樣依序執行，並非原子操作，取樣之間仍可能改變。失敗會在可寫入時記錄 `incomplete` 與原因；既有檔案不覆寫。部分記錄或截圖可能保留，Agent 須檢查。

2026-11-09 在授權環境對遊戲做一次唯讀測試。程序身分與前後 CG 幾何相符，AX 讀到一個視窗；綁定結果仍因缺少座標校準而為 `unknown`。此測試沒有送出輸入，也不能證明其他 App 可用。

實測原始紀錄（原始證據：來源專案 `screenshots/rust-joint-observation-20261109.png.binding.json`，未隨本庫攜帶）保留原型欄位值 `matched_samples_unverified`；正式介面改名為 `capture_identity_and_cg_geometry_matched_unverified`，明確說明只比較身分及 CG 幾何。紀錄中的圖片路徑仍指向當時暫存位置；保存的圖片（原始證據：來源專案 `screenshots/rust-joint-observation-20261109.png`，未隨本庫攜帶）是原始圖片的複本。

2026-11-09 新增 `geometry_hint`：當程序身分、目標前後幾何、AX 資料及取樣時間符合基本條件時，列出標題和幾何完全相同的 AX 路徑。零個或多個候選均如實列出。資料不足時為 `unknown`。這只是原始資料的未校準比對，不檢查清單完整性、遮擋及座標系是否一致，不是視窗綁定，路徑也不可用於操作。

同日對遊戲做一次唯讀觀察，程式列出一個幾何與標題相同的候選。主結果仍是 `calibration_missing`、`unknown` 和 `action_allowed=false`；此結果不證明候選就是同一視窗。

2026-11-09 新增 `capture_geometry`。它檢查擷取類型、視窗與內容矩形、縮放值，以及縮放後向上取整的圖片尺寸。資料相符時回報 `metadata_consistent_unverified`，缺資料或不符時回報 `unknown`。這只比對 helper 回傳的資料，不校準螢幕、AX 或滑鼠座標，也不證明圖片內容與視窗一致。

同日對 iPad 延伸螢幕上的遊戲視窗做一次唯讀測試，擷取資料的縮放值為 2，尺寸與矩形資料相符。主結果仍是 `unknown`，座標校準仍缺失；沒有送出輸入。

2026-11-09：AX／CG 比對改用只讀頂層視窗的 helper 模式，不遍歷子元素。模式以 `tree_scope=windows_only` 標示；它仍檢查頂層資料完整性和程序身分。舊 helper 無此模式時會停止，必須先重新執行 `sh build-ax.sh`。一般 `accessibility` 查詢仍使用原本的受限子樹。

同日唯讀測試遊戲與另一個 App。兩者的頂層視窗清單都沒有因子樹而截斷。遊戲有一個幾何候選；另一個 App 有視窗缺少標題，候選仍為未知。兩者均缺座標校準、禁止操作。這不是對所有 App 的驗證，也沒有保存另一個 App 的標題或內容。

2026-11-09：`ax_field_diagnostics` 列出缺少或截斷標題的視窗數、缺少幾何的數量，以及未校準矩形的重疊數。無法比較時重疊數為 `null`。它不列出新增的標題或座標，也不改變綁定結果。對另一個 App 的唯讀測試中，一個無標題視窗的原始矩形與目標重疊；因座標系未校準，這不是實際遮擋的證明。主結果仍為未知。

## AX 資料不完整時的回報

`live_ax_cg::observe` 一律不帶校準。取樣全部完成時，頂層 `reason` 恆為 `calibration_missing`，AX 資料不完整的原因記在 `findings`，其他可用資訊照常保留（`ax_windows`、`ax_field_diagnostics`、`geometry_hint`、`cg_before_target`、`evidence`）。這類結果都是 `status=unknown`、`binding=unverified`、`action_allowed=false`。任何一個取樣呼叫失敗（例如 `failed_stage=accessibility`）時則改回 `reason=observation_failed`，沒有 `binding`、`ax_windows` 等欄位，只保留 `evidence` 裡已取得的取樣。

| 情況 | `findings` 或 `reason` | 保留的資訊 |
|---|---|---|
| AX 結果的 `scope` 不是 `process_ax_windows`、`cg_window_binding` 不是 `unverified`、整體 `truncated`、缺 `windows` 陣列、`window_count_reported` 與實際視窗數不符 | `ax_snapshot_incomplete_or_unsafe` | 已取得的視窗摘要（缺 `windows` 時為空陣列）、`evidence.ax_windows`、CG 目標視窗、顯示器與程序身分證據 |
| AX 視窗缺 `title`、`frame` 或 `role`，`role` 不是 `AXWindow`，`title_truncated` 或 `role_truncated`，標題為空白，`frame` 無效（零尺寸或非有限值），`owner_pid` 不符，`path` 與位置不符 | `ax_window_fields_unverified` | 摘要 `ax_windows` 保存解析後的值（格式錯誤的 `frame` 或 `path` 會變成 `null` 或空陣列），原始值仍在 `evidence.ax_windows.windows`；`ax_field_diagnostics` 列出缺少或截斷標題的數量與未校準的重疊數 |
| AX 結果的程序身分無法確認（例如 `status` 為 `error`，即使仍有視窗；`status` 本身不會觸發 `ax_snapshot_incomplete_or_unsafe`），或前後身分不同 | `process_identity_unverified_or_changed` | 擷取前的身分與其他取樣仍保留在 `evidence` |
| 兩個以上 AX 視窗的標題與幾何都和 CG 目標完全相同 | 不是錯誤；`geometry_hint.status` 為 `raw_exact_matches`，`candidate_count` 大於 1 | 全部候選的 `ax_paths` 都列出，不選擇其中一個 |
| 取樣呼叫失敗（AX 呼叫失敗時 `failed_stage=accessibility`） | `reason=observation_failed`、`error`、`failed_stage` | `evidence` 內已取得的 `cg_before`、`display_before`、`process_identity.before` 等；沒有 `binding`、`ax_windows`、`geometry_hint` |

`raw_exact_matches` 只在程序身分、快照與每個 AX 視窗都通過驗證時才出現，此時 `candidate_count` 可為 0、1 或更多；否則 `geometry_hint.status` 為 `unknown`。重複候選只列出、不解析；`ax_paths` 與座標是未校準的原始資料，不可用於操作。`ax_cg::match_candidate` 在有校準時對同樣的缺陷回報 `ax_snapshot_incomplete_or_unsafe`、`ax_window_incomplete_or_invalid`、`cg_target_title_missing` 或 `multiple_exact_ax_candidates`（`Ambiguous`），也一律 `action_allowed=false`。

座標校準的離線擬合見 [CALIBRATION.md](CALIBRATION.md)；目前 `live_ax_cg::observe` 仍不帶校準。
