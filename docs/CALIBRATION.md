# 座標校準擬合與失效條件

`src/calibration.rs` 提供純離線的校準擬合。它不解碼圖片、不呼叫 UI、不送輸入。標記的像素座標由呼叫端提供（實機驗證子 issue 負責取得）；本模組只做數學，並說明這個結果何時失效。

## 方法

呼叫端提供至少 3 個不共線的標記配對：標記在視窗內容中的點座標 `point`，以及在擷取圖片中量到的像素座標 `pixel`。另需視窗框（點，全域座標，可為負值）、`point_pixel_scale`、顯示器摘要 `display_sha256`（64 位小寫十六進位）與取樣時間。

模型是逐軸的 `pixel_x = scale_x * x + offset_x`、`pixel_y = scale_y * y + offset_y`，兩軸各自用最小平方法擬合。擷取圖片以視窗左上角為原點，所以視窗左上角經擬合後應落在像素 `(0,0)` 附近；左側或上方螢幕的負原點同樣適用。

## 門檻

| 常數 | 值 | 意義 |
|---|---|---|
| `MIN_MARKERS` / `MAX_MARKERS` | 3 / 64 | 標記數量範圍 |
| `MAX_RESIDUAL_PX` | 1.5 | 每個標記實測與預測像素的最大距離 |
| `MAX_SCALE_REL_ERROR` | 0.01 | 擬合縮放與 `point_pixel_scale` 的相對誤差 |
| `MAX_ORIGIN_ERROR_PX` | 2.0 | 視窗左上角擬合後離 `(0,0)` 的最大距離 |
| `MIN_NONCOLLINEAR_AREA` | 1.0 | 判斷共線的最小叉積（點座標平方） |

## 輸出

成功時 `status=calibration_fitted_unverified`，欄位包含 `schema_version`、`status`、`action_allowed`、`marker_count`、`point_pixel_scale`、`window_frame_points`、`display_sha256`、`fit`（`scale_x`、`offset_x`、`scale_y`、`offset_y`）、`residuals_px`、`max_residual_px`、`max_residual_threshold_px`、`max_age_seconds`、`invalidated_by`、`note`、`evidence_id`。`evidence_id` 是其餘欄位（不含 `evidence_id` 本身）以 `serde_json::to_vec` 緊湊序列化（鍵依字母排序）後的 SHA-256，可重現；它只識別內容，不含取樣時間。

不能擬合時 `status=unknown`，`reason` 為下列之一：

| `reason` | 條件 |
|---|---|
| `too_many_markers`、`markers_insufficient` | 標記多於 64 個、少於 3 個（或沒有標記） |
| `non_finite_input` | 座標、視窗框或縮放不是有限值，或有限的極端輸入讓擬合結果溢位成非有限值 |
| `invalid_scale`、`invalid_window_frame`、`invalid_display_digest` | 縮放不在 (0,16]、視窗框寬高不為正、顯示器摘要格式錯誤 |
| `marker_outside_window`、`marker_pixel_negative` | 標記點在視窗框外、像素為負 |
| `markers_collinear` | 以第一個標記為基準，所有標記對的叉積絕對值都小於 `MIN_NONCOLLINEAR_AREA`（含共線、近乎共線與重複點） |
| `residual_exceeds_threshold` | 最大殘差超過 `MAX_RESIDUAL_PX` |
| `fitted_scale_mismatch` | 擬合縮放與 `point_pixel_scale` 不符 |
| `frame_origin_mismatch` | 視窗左上角擬合後不在像素原點附近 |
| `evidence_hash_unavailable` | 原生 SHA-256 不可用（非 macOS） |

## 失效條件

`invalidated_by` 列出下列四項，`Calibration::check` 依序判斷，任何一項成立就失效：

1. `display_changed`：顯示器摘要不同。
2. `window_frame_changed`：視窗框不同（移動或縮放都算）。
3. `scale_changed`：`point_pixel_scale` 不同。
4. `calibration_expired`：校準年齡超過 `ax_cg::MAX_CALIBRATION_AGE`（60 秒；剛好等於不算過期）。現在時間早於校準時間時為 `calibration_time_invalid`。

`check_json` 回報 `not_invalidated_unverified` 或 `invalidated`。「未失效」不代表校準正確，只代表上述四項沒有改變。

## 限制

- 「圖片尺寸與縮放吻合」（`capture_geometry`）本身不是校準；沒有標記配對時一律是 `unknown`（`markers_insufficient`）。
- 門檻比較一律 fail-closed：NaN 或無限值不會通過任何門檻。
- 擬合結果不證明視窗綁定、遮擋、畫面新鮮度或標記偵測正確，也不授權輸入。
- `action_allowed` 恆為 `false`。模組沒有公開的點到像素轉換函式，輸出也沒有可操作的座標或路徑。
- 目前 `live_ax_cg::observe` 仍不帶校準；把校準接進觀察流程屬於實機驗證子 issue。
