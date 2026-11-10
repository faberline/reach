# 背景輸入評估

目前沒有啟用輸入後端。`action` 固定回報不支援。

## 候選方案

[trycua/cua-driver](https://github.com/trycua/cua/blob/main/libs/cua-driver/README.md) 的核心驅動採 MIT 授權。它提供 Rust SDK 和 C ABI。它是獨立專案，不是本框架或 OpenAI Computer Use。其 macOS 背景視窗輸入使用非公開的 SkyLight 介面。上游指出，部分遊戲可能忽略事件，或使用實體游標的座標。

可重現的研究基準是 [cua-driver-rs-v0.41.0](https://github.com/trycua/cua/releases/tag/cua-driver-rs-v0.41.0)，對應 commit `f3e4c43b8a3bc13c5bdb9b6e248801d79af44346`。這是候選版本，不代表已安裝或已通過驗證。Rust SDK 的 [Cargo 設定](https://github.com/trycua/cua/blob/cua-driver-rs-v0.41.0/libs/cua-driver/rust/crates/cua-driver-sdk/Cargo.toml) 使用同一專案內的相對路徑依賴，且不發布至 crates.io。若評估二進位，必須使用同一固定版本的函式庫和標頭，並驗證發布檔的雜湊與簽章。不要直接執行上游安裝器：它不只是下載檔案，還會處理權限等設定。

該版本的 [ClickInput](https://github.com/trycua/cua/blob/cua-driver-rs-v0.41.0/libs/cua-driver/rust/crates/cua-driver-contract/src/inputs.rs) 有指定 PID、window ID 和背景模式的欄位，但座標使用驅動擷取圖片的像素。框架目前的離線檢查使用視窗點數；高解析度螢幕不能直接混用。[macOS 背景左鍵的實作](https://github.com/trycua/cua/blob/cua-driver-rs-v0.41.0/libs/cua-driver/rust/crates/platform-macos/src/tools/click.rs) 說明目標 App 可能被啟動，然後嘗試恢復前景。上游也指出原生選單可能短暫切換前景或鍵盤焦點。因此，此路徑不符合目前要求的全程不切換焦點，不能直接啟用。

驅動的 `capture_id` 是選填。只有使用該驅動產生的 ID，才能走它自己的畫面新鮮度及視窗綁定檢查。本框架的圖片 SHA-256 並不是同一種 ID。若將來研究其他可行輸入路徑，必須處理兩份擷取的時間與內容對應，並強制驗證畫面身分。

Apple 的 [Remote Desktop 說明](https://support.apple.com/en-bw/guide/remote-desktop/apd4f46319e/mac) 提供以另一個帳號登入虛擬顯示的方式，現場使用者可以繼續工作。這是另一個帳號的桌面；文件沒有證明它能控制目前帳號裡已開啟的遊戲，也沒有證明游標與焦點的完整隔離。[高效能螢幕共享說明](https://support.apple.com/or-in/guide/mac-help/mchl1883115d/27/mac/27) 則指出，以現場相同帳號登入時實體螢幕會變黑，無法符合目前的同時使用需求。

Electron 的[上游測試與修改](https://github.com/electron/electron/pull/54493) 使用帶目標 window number 的 `NSEvent` 轉為 `CGEvent`，再送至指定 PID。該測試對特定 Electron 視窗觀察到背景點擊，並非對所有 App 或遊戲的證明。本專案先前的 PID 定向測試事件沒有有效 window number，目標控制項未收到點擊。未來可在自建測試程式比較帶 window number 的事件，但目前尚未驗證，也不得改用會移動實體游標的全域事件。[Apple 的 AXPress 說明](https://developer.apple.com/documentation/applicationservices/1462091-axuielementperformaction?language=objc) 只說明向元素請求動作，沒有保證焦點不變。

## 2026-11-08：定向事件的限制

重新核對 [cua-driver v0.41.0 macOS 說明](https://github.com/trycua/cua/blob/cua-driver-rs-v0.41.0/libs/cua-driver/rust/Skills/cua-driver/MACOS.md)：上游描述一種不提高視窗層級的背景點擊，使用私有的 SkyLight 程序定向事件和目標視窗的焦點記錄。它聲稱一般情況下使用者的前景視窗保持鍵盤焦點；這是上游說明，不是本專案實測。

同一份說明也指出，原生選單可能短暫提高視窗，部分 App 可能自行啟動；驅動之後嘗試恢復。其 [v0.41.0 點擊程式碼](https://github.com/trycua/cua/blob/cua-driver-rs-v0.41.0/libs/cua-driver/rust/crates/platform-macos/src/tools/click.rs) 也有目標變成前景後重新啟動原 App 的處理。恢復焦點不能證明過程完全沒有切換，因此不能把此路徑一律視為符合使用者要求。

上游另指出，某些遊戲或繪圖視窗可能過濾程序定向事件，或依賴實體指標。這是待測風險，不證明特定遊戲必定拒絕事件。上游建議的前景備援會移動實體游標或切換前景，本專案不得自動使用。具體 App、系統版本和每種事件仍需分開驗證。

上游 [issue #2874](https://github.com/trycua/cua/issues/2874) 記錄過同一動作經兩條傳送路徑產生重複事件的問題。未核實該問題在 v0.41.0 的完整修復狀態。未來後端不得因送達結果未知而靜默改用另一條路徑或重試；要保留未知狀態，交由 Agent 依畫面確認。

## 整合條件

1. 預設不啟動或連接驅動。先用假後端測試封裝。
2. 每次操作都指定程序與視窗，並拒絕 desktop 目標和 foreground 模式。操作前後重查程序身分、視窗及畫面。
3. 座標或元素必須綁定當次擷取。畫面過期、判讀不明、身分不符或無法驗證時停止。驅動回報成功不代表 App 已完成操作。
4. 先在自建測試 App 驗證操作結果，並檢查實體游標、前景 App、焦點及視窗順序。不能只測驅動回傳值。
5. 測試 App 成功後，仍須獨立驗證每個目標 App。遊戲畫布可能忽略背景事件，或讀取實體游標位置。不能因為測試 App 成功就宣稱遊戲可用。

## 視窗編號的待驗證假設

2026-11-09 重新研究程序定向事件。[其他專案的研究筆記](https://github.com/rlmsinclair/claude-on-second-display/blob/main/docs/pid-posting-research.md)提出，滑鼠事件的視窗欄位可能影響部分 App 的路由；這是該專案的觀察，不代表本機或遊戲有效。本機 macOS SDK 的 `CGEventTypes.h` 列出欄位 91 和 92。Rust 測試使用虛構視窗編號，對記憶體中的滑鼠事件寫入並讀回這兩個欄位，且核對兩者可分別改變；沒有查找視窗或投遞事件。此結果只證明本機可儲存欄位，不證明送達、視窗身分、座標、游標或焦點隔離。不得將此測試當作啟用輸入的依據。

[Cua Driver 上游問題](https://github.com/trycua/cua/issues/2874)記錄不同事件傳送路徑可能讓同一次點擊重複送達；即使某傳送呼叫返回，也不代表 App 已處理。未確認是否送達時，不應自動切換路徑重送。其他專案也描述可能改變目標視窗的鍵盤焦點或加入修飾鍵；這些行為不符合本專案目前已驗證的範圍，不能直接採用。

Apple 的 [`NSWindow.windowNumber` 文件](https://developer.apple.com/documentation/appkit/nswindow/windownumber?language=objc) 區分 App 內的視窗裝置編號與 WindowServer 的全域編號。文件沒有說兩者的數值必定不同，也不能直接把兩者當成同一種識別。`TargetEventExperiment.swift` 現在分開記錄子程序回報的 AppKit 編號及 CG 視窗 ID；它以子程序 PID、含隨機識別字的標題和子程序回報的畫面幾何，尋找唯一的 CG 視窗。清單重複、資料缺失或幾何不符時停止。這只是自有 fixture 的關聯方式，尚未實機驗證；不能推廣到一般 App 或隱藏視窗。畫面座標換算也需要實機核對。

離線自測以合成視窗編號建立 `NSEvent`，轉成 `CGEvent` 再轉回 `NSEvent`，確認本機執行環境的事件編號往返仍相同。過程不建立視窗或投遞事件。這不能證明 WindowServer 使用該編號路由事件，也不能證明其他程序會收到事件。

## 無視窗程序事件流實驗原型

`sh build-headless-event-fixture.sh` 只編譯並執行純資料自測，不啟動子程序、不建立 tap、視窗或事件。執行檔未帶參數會停止。原型沒有連接 `action`。

`--sample-only` 只取樣，不啟動子程序或送出事件。它先讀取螢幕視窗順序，再讀取系統焦點；清單無法讀取或欄位不完整、焦點失敗或狀態變動時回報未知。程式不能證明系統清單涵蓋所有視窗。清單與游標資料只留在記憶體，不寫入結果。一次取樣成功不能保證未來取樣或輸入安全。

2026-11-09 09:23（台北），依使用者同意執行一次 `--run-experiment`。14 筆基準取樣沒有讀取錯誤，焦點、前景、游標和視窗順序取樣一致；但硬體修飾鍵旗標有非零值，程式在基準檢查停止，`action_attempted=false`，沒有投遞事件。程式只保存「是否為零」，不能從結果判定是哪個旗標或是否有按鍵被按住。清理回報完成。此次未證明投遞或輸入隔離，也未自動重試。原始結果與程式（原始證據：來源專案 `benchmark/headless-input-20261109-092317/`，未隨本庫攜帶）

取樣會比較系統焦點 App 在焦點元素查詢前後的身分，並要求該 App、焦點元素和前景 App 的程序相同；不一致時停止。Apple 指出 `NSRunningApplication` 的變動屬性可能受主執行迴圈影響而過時，因此這些連續查詢仍不能證明全程焦點不變。[Apple 文件](https://developer.apple.com/documentation/appkit/nsrunningapplication)

只有明確執行 `target/debug/mac-headless-event-experiment --run-experiment` 才會嘗試實驗。父程序只啟動同一執行檔的受控子程序，不接受外部目標 PID 或視窗。子程序建立 tap 前，先核對父子關係、雙方執行檔身分，以及受控父程序在私有目錄寫入的一次性握手。握手綁定 nonce、標記、父程序 PID、啟動時間及執行檔。接著子程序設定 `.prohibited`，不建立視窗；只建立針對自身 PID 的 `listenOnly` 事件 tap。父程序檢查本機權限、子程序 PID、父子關係、程序啟動時間及執行檔路徑，並核對 nonce 與 tap 就緒資料。所有條件通過且取樣狀態穩定後，最多對該子 PID 發送一個帶隨機標記的 `mouseMoved` 事件；沒有全域投遞或滑鼠按鍵。任何檢查或收據驗證失敗都回報失敗。父程序等待子程序停止後才嘗試清理私有目錄，並回報清理結果；未停止或移除失敗均不能視為成功。

這不是已核准的安全背景輸入。單一 `mouseMoved` 事件能否送達無視窗程序、標記能否保留及其副作用均未驗證。子程序收據最多證明有限時間內的事件流，不證明 `NSView` 收到、遊戲可操作，或其他 App 可用。檢查程序身分與投遞事件之間仍有無法消除的競態。游標、焦點和前景取樣可能漏掉瞬間變化，也無法區分使用者自行移動；實體輸入不受影響亦未證明。結果不儲存使用者游標座標。來源使用私有狀態表並將抑制期間設為零、允許本地事件，這些設定不能代替隔離實測。程序若被強制終止，清理程式可能無法執行，私有目錄或子程序可能殘留。[來源狀態](https://developer.apple.com/documentation/coregraphics/cgeventsourcestateid)、[本地事件設定](https://developer.apple.com/documentation/coregraphics/cgeventsource/setlocaleventsfilterduringsuppressionstate%28_%3Astate%3A%29?language=objc)、[禁止視窗及啟用](https://developer.apple.com/documentation/appkit/nsapplication/activationpolicy-swift.enum/prohibited?changes=_6__5&language=objc)。

2026-11-09 核對 Apple 文件：`privateState` 建立獨立的事件狀態表，用來追蹤該來源的按鍵、滑鼠按鈕和修飾鍵狀態。這不等於建立另一個畫面游標，也不是焦點或實體輸入隔離的保證。[Apple 狀態表說明](https://developer.apple.com/documentation/coregraphics/cgeventsourcestateid)、[程序定向投遞介面](https://developer.apple.com/documentation/coregraphics/cgevent/posttopid%28_%3A%29)。

2026-11-09 的 headless 實驗原始碼另檢查 HID 狀態表中左、右、中滑鼠按鍵與修飾鍵旗標。取樣中若任一按鍵被按住或旗標非零，實驗停止；取樣資料只留在記憶體。這不涵蓋其他滑鼠按鍵或一般鍵盤按鍵，也可能漏掉取樣間的操作，不能證明使用者閒置或輸入隔離。此改動只做編譯與離線自測，尚未執行事件實驗。

取樣失敗時，實驗結果另記固定分類的數量，例如焦點不可用、前景或游標不可用，以及資料無效。它不輸出視窗名稱或游標位置。分類只說明取樣失敗的階段，不判定根因。此改動只通過離線自測，尚未執行事件實驗。

監測器記錄每筆取樣的單調時間。實驗只在事件前後都有新鮮取樣，且事件後至少完成三筆取樣時，才可回報取樣狀態未變。取樣間隔、單筆取樣耗時、事件耗時或事件兩側空檔超過 250 毫秒時也拒絕。此門檻僅用來排除明顯缺口，未經實測校準；間隔內仍可能發生未觀測的變化。事件投遞結果只有在這些檢查及其他實驗條件全通過時才回報完成。上述情況均不代表輸入安全。

Apple 對 [AXUIElementPerformAction](https://developer.apple.com/documentation/applicationservices/1462091-axuielementperformaction?language=objc) 的說明指出，無法完成的回報可能來自逾時，也不一定表示動作失敗。不可因這種回報就自動重試。[CGEventPostToPid](https://developer.apple.com/documentation/coregraphics/cgevent/posttopid%28_%3A%29?language=objc) 的文件列出指定程序的介面，未保證游標和焦點隔離或目標 App 接收事件。兩者仍須各自驗證。

此握手用來限制意外啟動與混淆，不是抵抗同一 macOS 帳號下惡意程序的安全邊界。nonce 與標記用來關聯單次實驗，不應作身分驗證金鑰。

`sh test-headless-event-fixture.sh` 另檢查空參數、外部 PID 參數、不完整子程序參數，以及其他執行檔嘗試使用完整子程序參數時均遭拒絕。最後一項會啟動測試程序，但會在查權限及建立 tap 前停止。這些檢查都不啟動實驗。

2026-11-08 只編譯及執行上述離線測試。尚未啟動實驗、建立 tap 或發送事件。

來源：[macOS 背景操作限制](https://github.com/trycua/cua/blob/cua-driver-rs-v0.41.0/libs/cua-driver/rust/Skills/cua-driver/MACOS.md)、[上游安裝器](https://github.com/trycua/cua/blob/cua-driver-rs-v0.41.0/libs/cua-driver/scripts/_install-rust.sh)。

## Rust 協調的無視窗事件測試

`cu-headless-check --evidence-dir /absolute/new-directory` 是明確啟動的受控測試。先建置 `cu-headless-check`、`cu-focus-watch`、`mac-focus-notifications` 及 `mac-headless-event-experiment`。程式只使用與自身同目錄的固定程式，不接受外部 PID、視窗或座標。它先等焦點監看 ready，再啟動既有測試。測試最多對自身子程序送一個帶標記的 mouseMoved。

Rust 將監看得到的前景 PID 與程序啟動時間傳給事件測試。測試在發送前核對身分，Rust 也在結束後再次核對。Rust 核對事件收據的 PID、標記和 nonce，以及焦點取樣、監看結束和程序清理回報。監看變更、資料不符、逾時或失敗時停止；若測試已啟動但結果不明，回報 `trial_delivery=unknown`，不自動重試。新目錄保存 `result.json` 及 `focus.jsonl`。中斷時子程序的後代清理可能無法確認，Agent 須檢查。

這個測試有競態：焦點通知可能延遲，程式收到通知前事件可能已送出；取樣也可能漏掉變化。成功只證明受控無視窗事件的當次結果，不能證明有視窗 App、遊戲或實體輸入隔離安全。`action_allowed` 和 `input_isolation_verified` 永遠為 false，未接入一般操作。

## 隱藏 AppKit 程序的受控事件

`cu-hidden-appkit-check --evidence-dir /absolute/new-directory` 是獨立的明確啟動測試。它使用同目錄的固定 helper，在焦點監看下，向自身子程序發送最多一個帶標記的 mouseMoved。子程序建立但不顯示 AppKit 面板，並分別記錄程序事件 tap 與 AppKit 本地監看的收據；沒有按下滑鼠或鍵盤。呼叫端不能指定外部 PID、視窗或座標。

若事件已嘗試但收據、取樣或清理不完整，helper 保留私有目錄及已存在的收據，並回報路徑；Rust 將送達狀態保留為 unknown，不自動重試。事件與通知仍有競態。實測中的 AppKit 收據 `window_number=0`，因此只表示事件到達該 App 的本地監看，不能證明隱藏視窗或其控制項收到事件，也不能推論其他 App、遊戲或實體輸入安全。

2026-11-09 修正三個受控測試的協調程式：子程序一旦啟動，就先在記憶體標記 `trial_started=true`、`trial_delivery=unknown`。若無法取得輸出管道，仍保留程序控制代碼供共用清理流程處理，不再誤報未啟動。假 helper 的異常輸出測試確認會停止並保留 unknown；輸出管道本身無法取得的分支尚未實際注入測試。協調程式若在保存結果前崩潰，仍可能沒有最終紀錄，不能把缺少紀錄當成沒有送達。

三個 Rust 協調程式現在在啟動測試子程序前，以新檔建立並同步 `attempt.json`。若檔案無法寫入，停止且不啟動子程序。若最終 `result.json` 缺失，事前紀錄提醒 Agent：送達狀態未知；不可自動重試。此檔建立在啟動之前，存在不代表子程序真的啟動或事件真的送出。它不保證作業系統崩潰、斷電、檔案系統故障或協調程式與後代程序清理的結果。

使用假子程序測試中途終止：在假子程序已啟動後停止協調程式，事前紀錄仍存在，且最終結果檔不存在。測試沒有送出系統輸入；這只覆蓋無視窗協調程式的一次中斷，不代表真實事件或所有故障情況都已驗證。

2026-11-09：三個受控測試 helper 加入虛擬鍵碼 0 至 127 的 HID 狀態取樣，只保留是否任一鍵按住；不保留個別鍵碼或文字。Rust 需要 `keyboard_keys_clear=true` 才能把實驗視為完成；舊版 helper 缺少欄位會停止。一次唯讀基準回報 15 筆均未看到受檢鍵按住，沒有投遞事件。測試紀錄（原始證據：來源專案 `benchmark/keyboard-state-sample-20261109/README.md`，未隨本庫攜帶）。按鍵按住時的真實偵測、其他裝置與取樣間的活動均未驗證；API 也未提供本程式可確認的完整性保證。

2026-11-09：三個受控協調程式在事前紀錄同步完成後，先等最多 150 毫秒的監看訊息，再確認監看程序仍在執行，才啟動測試子程序。收到訊息或監看失效就停止。假監看程式在紀錄建立後送出焦點變更時，三個協調程式均在啟動測試前停止；已有的事前紀錄仍保留。150 毫秒不是安全門檻：通知可能延遲或遺漏，等待結束後仍有競態。此測試未投遞系統輸入。

## 隱藏視窗未收到 view 事件的診斷限制

2026-11-09 檢查自有隱藏視窗測試：視窗已設 `acceptsMouseMovedEvents=true`，但測試 view 沒有建立 tracking area；事件位置採當時實體游標位置，不保證位於測試 view 內。Apple 文件說明 [`acceptsMouseMovedEvents`](https://developer.apple.com/documentation/appkit/nswindow/acceptsmousemovedevents) 控制視窗是否接受並分派滑鼠移動事件；[`NSTrackingArea`](https://developer.apple.com/documentation/appkit/nstrackingarea) 可依範圍和啟用條件向 owner 發送移動事件。這些是未收到 view 事件的可能因素，不是已證實的原因；隱藏視窗或其他 AppKit 路由條件也可能影響結果。舊證據沒有保存事件位置與 view 的關係，無法事後判定。

自有測試程式的 AppKit 收據現在可記錄事件轉換後是否落在自有 view 範圍內，以及 view 的 tracking area 數量；不記錄座標。視窗編號不符或座標無效時，範圍結果為 null。這只是幾何診斷，不等於 AppKit hit test、事件分派或使用者游標隔離。新增程式只通過編譯及純資料測試；沒有重跑事件實驗，也沒有改變輸入禁用狀態。

## 本機 Codex Computer Use 唯讀檢查

以下為對外部產品的唯讀觀察，Reach 不依賴它。

2026-11-09 唯讀程序清單顯示 `Codex Computer Use.app` 的 `SkyComputerUseService` 正在執行。App 的 bundle ID 是 `com.openai.sky.CUAService`，版本是 `26.1102.1001824`。其 Mach-O 直接連結清單包含 ScreenCaptureKit、ApplicationServices 與 IOKit；未定義符號清單含 `AXUIElementPerformAction`、`CGEventGetFlags` 與 `CGSMainConnectionID`。這些僅表示執行檔的連結或符號參照，不證明實際呼叫路徑或輸入隔離。唯讀查詢未看到同名 Client 或 `screensharingd` 程序；這不排除動態載入、其他程序或不同名稱的遠端機制。

2026-11-09 再以插件目錄搜尋「computer use」與「remote desktop」，結果沒有提供可用的本機 Computer Use 介面；此搜尋不是完整目錄。目前工具清單沒有可呼叫的 Computer Use 操作介面。不得以執行檔存在、符號或使用者先前的經驗，推定它不搶游標和焦點，也不可直接連接私有服務或繞過授權。後續如有正式可用介面，可在相同條件下先做唯讀比較，再依專案規則驗證輸入。此次沒有截圖或發送輸入。
