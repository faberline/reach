# 隔離工作階段研究

Apple 的 [Remote Desktop 說明](https://support.apple.com/en-om/guide/remote-desktop/apd4f46319e/mac) 描述：以不同使用者帳號認證時，可連線到該帳號的虛擬桌面，而現場使用者繼續使用自己的桌面。是否適用這台 Mac、現有權限及工具，仍須實測。不要建立帳號、啟用遠端管理或取得密碼作為隱含步驟。

這是隔離測試 App 的候選環境，不是控制目前帳號遊戲的解法。[Quartz Window Services](https://developer.apple.com/documentation/coregraphics/quartz-window-services) 描述視窗清單屬於目前使用者工作階段；因此推論另一登入工作階段不能直接把本帳號遊戲視窗當成自己的視窗。還需分別驗證程式、權限、擷取與輸入。Apple 的[高效能螢幕共享說明](https://support.apple.com/en-ie/guide/mac-help/mchl1883115d/27/mac/27)指出，以本機正在使用的相同帳號登入時，硬體螢幕會變黑；不得把它當成不干擾使用者的方式。

2026-11-09 唯讀查詢本機目錄中 UID 501 至 59999、名稱不以底線開頭的帳號：符合條件的只有目前帳號，沒有另一個符合條件的帳號。這是有限篩選，不排除其他來源或特殊帳號，也不證明任何帳號可供遠端登入。沒有保存帳號名稱、建立帳號或修改共享設定。第二帳號若由使用者提供，只能先測隔離桌面的自有測試 App；它不能直接解決目前帳號遊戲的輸入。

使用者隨後表示沿用目前帳號，該帳號就是測試用途。不要再把建立第二個帳號當成既定下一步。仍須遵守不搶用使用者滑鼠與焦點的要求；目前帳號作為測試帳號不代表同帳號螢幕共享已隔離，也不等於允許未驗證的背景輸入。

## 唯讀狀態查詢

在儲存庫根目錄執行 `sh build-session-status.sh` 編譯 helper，不開視窗或投遞事件。`cu-agent` 的 `session_status` 需要絕對路徑 `session_helper`。它回報 `dictionary_available`、`on_console` 和 `login_done`，不回報帳號名稱或使用者 ID。

以下 `<REACH_DIR>` 代表本儲存庫在呼叫者環境中的絕對路徑。

```json
{"id":"session","method":"session_status","session_helper":"<REACH_DIR>/target/debug/mac-session-status"}
```

結果固定 `unknown`、狀態碼 3、`action_allowed=false`。旗標不能識別具體工作階段，不能確認鎖定狀態，也不能證明隔離、可見視窗或輸入安全。helper 是呼叫端提供的可信程式。

2026-11-09 在目前工作階段唯讀測試：字典可讀，`on_console=true`、`login_done=true`。沒有發送輸入；這不證明其他工作階段的結果。

## 工作階段身分限制

[CGSessionCopyCurrentDictionary](https://developer.apple.com/documentation/coregraphics/cgsessioncopycurrentdictionary%28%29) 只提供呼叫程序的視窗伺服器工作階段資訊；Apple 列出的[標準欄位](https://developer.apple.com/documentation/coregraphics/window-server-session-properties)沒有提供此框架可用來確認目標視窗與呼叫端屬於同一圖形工作階段的唯一識別。`on_console` 和 `login_done` 也不是工作階段身分。

[SessionGetInfo](https://developer.apple.com/documentation/security/sessiongetinfo%28_%3A_%3A_%3A%29) 可查安全工作階段 ID，但安全工作階段不等同已驗證的視窗伺服器工作階段；本機 macOS SDK 的 `Security.framework/Headers/AuthSession.h` 亦指出兩種概念不一定完全一致。不能拿此 ID 當作圖形視窗綁定或安全輸入的證據。通用框架的 `capabilities` 因此明確標示 `target_gui_session_binding=unverified`。
