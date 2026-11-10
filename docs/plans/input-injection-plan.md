# 真實輸入注入計畫（surface #96、#97 的 Safari 重測）

狀態：**草稿，待使用者審核**。2026-11-10 撰寫。

這份文件只是計畫。撰寫期間沒有送出任何滑鼠、鍵盤或 AX 動作事件，沒有執行會投遞事件的實驗入口，也沒有改任何權限。Reach 的現況不變：`action` 一律回報 `unsupported`，輸入停用（見 READMEï¼`README.md`ï¼、CAPABILITIESï¼`docs/CAPABILITIES.md`ï¼、AGENT-CLIï¼`docs/AGENT-CLI.md`ï¼）。本文提到的 issue 都是草稿，使用者核准前不在 GitHub 上建立。

## 0. 摘要

- **起因**：2026-11-10 使用者明確指示解除 faberline/reach#2 的唯讀限制並開始規劃。用途是 faberline/surface#96、#97 的真實瀏覽器重測（faberline/surface#99 決定由 reach 提供工具）。核准範圍只限這兩個 issue。
- **核心結論**：這兩個重測在語意上就需要**前景**輸入。#96 測的是「組字中點擊造成的焦點變化」，#97 測的是「候選窗在螢幕上的位置」；兩者都要求 Safari 在前景、握有鍵盤焦點、輸入經過 macOS 輸入法。reach#2 追求的「不搶游標與焦點的背景輸入」無法滿足它們，而且仍未驗證。
- **建議做法**：新增一個獨立、明確選用的「前景、有人在場」輸入模式，只經由 ReachAgent App，只對允許清單內的 bundle id 生效，一次一個事件，每個事件前後都有收據。reach#2（背景輸入）維持 `blocked`，不被這個模式取代，也不當它的備援。
- **最小能力**：指標移動、左鍵單擊、無修飾鍵的單鍵敲擊（鍵碼）。不做拖曳、修飾鍵、捲動、文字直接輸入、AX 動作。
- **最大的未知**：(1) 合成鍵盤事件在這台機器上是否確實經過注音輸入法並開出候選窗；(2) Safari 視窗的截圖像素到螢幕座標能否有足夠證據；(3) Surface 的確切步驟與它要 reach 取回的證據欄位。三者都排在對 Safari 送出第一個事件之前解決。
- **需要使用者在場**：建立簽章憑證與授權（reach#5、#6 既有的前置）、每一次真實事件實驗、最後的 Safari 重測。

## 1. surface #96、#97 實際需要的輸入

序列來源：2026-11-10 surface-service-desk 提供，與本計畫先前由 issue 內文推得的一致。**這份序列是對方從 issue 內文整理的，沒有人實際跑過**；其中關於 canvas 的敘述是對方對程式現況的解讀。仍缺的項目列在 1.4。

共同前置（不是輸入動作）：macOS、輸入來源 `com.apple.inputmethod.TCIM.Zhuyin`、showcase 用 surface `main` 的 dev build 在本機提供（`cargo xtask web dev showcase`，預設 `http://127.0.0.1:8080`）、WebGPU 啟用。UI 畫在單一 `<canvas>` 上，「Your name」與「Add one」不是 DOM 元素，Surface 還沒有瀏覽器無障礙樹（faberline/surface#54）；所以點擊只能用 canvas 內的座標，位置靠擷取或 OCR 找。另由唯讀查看 surface 程式得知：文字輸入經由一個隱藏的 textarea（`#surface-text-input`），頁面有 `#status` 文字（形如 `count=…;name=…;focused=…`）；這兩點 Surface 尚未確認。

瀏覽器範圍：#96 只測 Safari；#97 先 Safari，再 Chrome。兩者都不需要選候選字，也不需要按 Return 送出。

### 1.1 surface #96（組字中點擊）

| 步驟 | 輸入動作 | 鍵碼／目標 | 備註 |
|---|---|---|---|
| 1 | 左鍵單擊「Your name」欄位 | canvas 內座標 | 取得鍵盤焦點 |
| 2 | 敲 `1` | 鍵碼 18（`kVK_ANSI_1`），注音 ㄅ | 組字開啟，不送出 |
| 3 | 左鍵單擊「Add one」一次 | canvas 內座標 | 被測的那一下 |

驗收需要讀回：計數 0 → 1、`focused=increment`、ㄅ 保留、事件的 `isTrusted`，以及瀏覽器、OS、輸入來源。

### 1.2 surface #97（候選窗位置）

| 步驟 | 輸入動作 | 鍵碼／目標 | 備註 |
|---|---|---|---|
| 1 | 左鍵單擊「Your name」欄位 | canvas 內座標 | |
| 2 | 依序敲 `5`、`j`、`/` | 鍵碼 23、38、44（ㄓㄨㄥ） | |
| 3 | 敲空白鍵兩次 | 鍵碼 49 ×2 | 第一次定聲調，第二次開候選窗 |

步驟 3 之後不選字。Safari 要做三種情況，然後 Chrome 重複同樣步驟確認沒有退步：

| 情況 | 內容 | 額外動作 |
|---|---|---|
| (a) | 一般 | 無 |
| (b) | 頁面重新載入後的第一個候選窗 | 重新載入頁面（誰做見 S5） |
| (c) | 組字中按綠色視窗按鈕調整大小之後 | 單擊標題列綠色按鈕（語意見 S4） |

驗收需要讀回：候選窗視窗邊界（它是獨立的系統視窗，不屬於瀏覽器）、caret 的螢幕矩形（候選窗上緣須在 caret 下緣之下，容許 2 px），以及瀏覽器、OS、輸入來源。

### 1.3 所需動作的最小集合

1. 指標移動到視窗內一點（不按鍵）。
2. 左鍵單擊（同一點按下後立即放開）。
3. 單鍵敲擊：按下後立即放開，無修飾鍵。需要的鍵碼只有 18、23、38、44、49。序列本身不需要 Return；清理殘留組字若要用 Escape（53），須另列入鍵碼清單（見 S6）。

**不需要**：拖曳、右鍵、捲動、修飾鍵組合、直接插入 Unicode 文字、選候選字、按 Return 送出。

除了輸入，重測還需要三個唯讀能力：擷取瀏覽器視窗找出 canvas 內目標的位置；讀取輸入法候選窗（獨立系統視窗）的邊界；由擷取或其他唯讀方式取得 caret 的螢幕矩形。

### 1.4 Surface 的答覆與仍待確認的項目

已由 surface-service-desk 答覆（2026-11-10，未實跑）：

- S1（已答）：目標位置沒有 AX 可用，靠擷取或 OCR 在 canvas 內找。reach 提供擷取與 OCR；判讀哪個像素是「Add one」是呼叫端 agent 的工作，reach 不放 surface 專用邏輯。
- S7（已答）：#96 只測 Safari；#97 先 Safari 再 Chrome。網址預設 `http://127.0.0.1:8080`。Chrome 因此在重測範圍內，但加入允許清單仍需使用者核准（Q4）。

仍待確認：

- S2：`isTrusted` 怎麼讀回。Surface 表示需要頁面 console 或 debug 讀數。reach 不能讀瀏覽器 console，也不執行頁面 JavaScript；需要 Surface 把它顯示在畫面上（canvas 或 `#status` 之類的可見文字），reach 才能用擷取＋OCR 或 AX 讀到。計數與 `focused` 同理，需確認現有狀態顯示在畫面上看得到。
- S3：caret 的螢幕矩形怎麼量。Surface 的說法是由擷取畫面取得；caret 很細且會閃爍，從截圖量到 2 px 精度是否可靠待實驗。備案：隱藏 textarea 在 #95 之後是 caret 大小，它在瀏覽器無障礙樹的框可能就是 caret 矩形（推論，未驗證）；或由 Surface 在畫面上輸出數值。
- S4：「綠色按鈕調整大小」是進入全螢幕（預設單擊）還是縮放（Option＋單擊，或按住後選單）。Surface 的答覆沒有說明。全螢幕會換 Space，目標身分與校準都要重來；Option＋單擊需要修飾鍵，不在最小集合內。
- S5：情況 (b) 的重新載入由誰做。Cmd＋R 需要修飾鍵。建議第一版由在場的使用者手動重新載入。
- S6：每次重測後怎麼清掉殘留的組字（Escape，或重新載入頁面）。
- S8：Surface 端由誰驅動：呼叫 `cu-agent` 的 agent session，還是腳本。這影響一次一事件的節奏是否夠用。

## 2. 前景輸入與背景輸入的取捨

### 2.1 各路徑的可行性

「已知」指 Apple 文件或本庫既有紀錄支持；「待實驗」指必須在本機受控實驗才知道。沒有任何一格是本專案已實機驗證的。

| 路徑 | 指標點擊 | 鍵盤 | 經輸入法組字與候選窗 | 對使用者的影響 |
|---|---|---|---|---|
| A. 全域投遞（`CGEventPost` 到 HID／session tap） | 已知可送達前景視窗；瀏覽器收到的是系統事件，預期 `isTrusted=true`（待實驗確認） | 已知送往握有鍵盤焦點的 App | 普遍認為會經過目前的輸入法，**本機待實驗** | 會移動實體游標；目標必須在前景；使用者同時操作會互相干擾 |
| B. 程序定向（`CGEventPostToPid`） | 本庫舊實驗：沒有視窗編號的事件未到達控制項。Safari 是多程序架構，待實驗，預期低 | 待實驗 | App 不在前景時沒有作用中的輸入情境，預期**不可行**（待實驗） | 理論上不動游標；本庫未證明 |
| C. 帶視窗編號的 `NSEvent` → `CGEvent` 定向 | 上游只在特定 Electron 視窗觀察到；Safari 待實驗 | 不適用 | 同 B | 同 B |
| D. AX 動作（`AXPress`、設定 `AXValue`） | canvas 內控制項沒有 AX 元素，**不可行**；視窗綠色按鈕可行但不是被測的點擊 | 設定值會**繞過輸入法** | **不可行**（不產生組字事件） | 是否改變焦點未保證 |
| E. 直接附 Unicode 字串的鍵盤事件 | 不適用 | 可送字 | 繞過鍵位到注音的對應，**不可行** | 同 A |
| F. 私有 SkyLight 介面（cua-driver 類） | 上游聲稱可背景點擊，會短暫啟動目標再恢復 | 上游有描述 | 未知 | 使用非公開介面；不符合本專案規則 |
| G. 另一個登入工作階段／虛擬機／第二台 Mac 內走路徑 A | 同 A | 同 A | 同 A | 不干擾現場使用者，但需要另外的環境與授權；隔離工作階段研究ï¼`docs/ISOLATED-SESSION.md`ï¼中使用者已表示沿用目前帳號 |

### 2.2 為什麼這次必須是前景

- #96 的缺陷本身是「組字中點擊時 textarea 失焦」。只有 Safari 真的握有鍵盤焦點、輸入法真的在組字，這個行為才存在。
- #97 量的是候選窗在螢幕上的位置。候選窗由輸入法在作用中的輸入情境旁邊開啟；視窗不在螢幕上或 App 不在前景時，沒有東西可量。
- 兩個 issue 都明寫 mock 事件不算。路徑 D、E 產生的不是使用者會產生的事件序列。

因此本計畫只規劃**路徑 A**，並把它定義成獨立模式，不是背景輸入失敗時的備援。

### 2.3 與 reach#2 的關係

reach#2 的文字要求「不得使用全域滑鼠事件、前景備援或自動重試」。使用者 2026-11-10 的指示解除了唯讀限制，但 #2 的目標（背景、不搶游標與焦點）沒有變，也沒有因此被驗證。建議：

- reach#2 保持開啟與 `blocked`，範圍仍是背景輸入。
- 前景模式另開一組 issue（第 5 節），並在那裡記錄使用者的指示與範圍。
- 前景模式永遠不會被自動選用：請求必須寫明 `delivery: "foreground"`，背景請求失敗不會改走前景。
- `capabilities` 與 `describe` 分開回報 `foreground` 與 `background_window`；後者維持 `unsupported`。

### 2.4 必須先做的實驗（依序，前一項通過才做下一項）

1. **唯讀**：使用者自己用注音打字，reach 只觀察。確認候選窗屬於哪個程序、能否用 `all_windows` 類查詢取得邊界；Safari 的 `#status` 文字與隱藏 textarea 的框能否經 AX 讀到；輸入來源能否唯讀讀取。不送任何事件。
2. **自建 fixture，指標**：只對自己啟動的 fixture App 做移動、讀回、單擊；fixture 記錄收到的事件與座標。
3. **自建 fixture，鍵盤與注音**：fixture 的文字欄位記錄 marked text 與送出的文字；送 1.3 的鍵碼，確認組字、候選窗出現。這一步回答最大的未知 (1)。
4. **Safari**：前三項都有證據後，才把 Safari 加進允許清單。

任何一項失敗就停在那裡回報，不改走其他路徑。

## 3. 安全閘門

原則沿用既有文件：未知就停、不自動重試、不靜默降級、結果不明要留收據。下表列出每個閘門與可重用的既有程式。

| 閘門 | 內容 | 可重用 | 需要新增 |
|---|---|---|---|
| G1 明確啟用 | 預設停用。ReachAgent 必須由使用者以明確參數啟動（例如 `--enable-input --allow-bundle <id>`，可重複），並有到期時間與事件數上限；到期自動回到停用 | ReachAgent 的啟動參數與 `--test-mode` 慣例；`describe` 的 `agent` 欄位 | armed 狀態、到期、計數；`describe` 回報 |
| G2 允許清單 | 請求的目標 bundle id 必須在啟動時給的清單內；清單不能由請求修改；沒有萬用字元 | `find_bundle`、`process_identity`（兩次取樣、核心啟動時間） | 清單比對與拒絕碼 |
| G3 目標身分綁定 | 事件前後各取樣程序身分（PID、啟動時間、執行檔、bundle id）與視窗（ID、擁有者、layer 0、幾何）；任何不一致就停 | `Backend::process_identity`、單一視窗查詢、`input_guard::review` 的身分與幾何比對 | 在 ReachAgent 內執行同樣的檢查，緊貼事件投遞 |
| G4 內容綁定 | 目標是瀏覽器時，網址必須是本機 dev server（`localhost`／`127.0.0.1`；Surface 給的預設是 `http://127.0.0.1:8080`）；讀不到或不符就停 | AX 唯讀快照 | 讀網址欄位的規則（唯讀） |
| G5 觀察新鮮度 | 請求必須帶它依據的擷取（`image_sha256`、時間、視窗框）；超過期限或不是最新一張就拒絕 | `input_guard::Observation`／`ObservationToken`、`MAX_OBSERVATION_AGE`（10 秒）、`VirtualSession` 的序號與時間單調檢查 | 把呼叫端陳述換成 ReachAgent 自己保存的擷取紀錄 |
| G6 座標 | 只接受視窗內的點座標；換算成螢幕座標前，校準必須存在且未失效；點必須落在允許區域內（網頁內容區，或具名的標題列按鈕） | `calibration::fit`／`check` 與四個失效條件、`capture_geometry`、`display_inventory` | 允許區域規則；見 3.1 |
| G7 遮擋與最上層 | 依前後順序的視窗清單，目標點上最前面的 layer 0 視窗必須就是目標視窗 | `Backend::windows(None)` 的全程序前後順序清單、`ax_cg` 的重疊拒絕 | 點的命中檢查 |
| G8 前景與焦點 | 事件前目標 App 必須已是前景且握有焦點（reach **不**自己啟動或切換 App）；全程監看，焦點變更就停 | `mac-focus-notifications --watch`、`cu-focus-watch`（ready、`focus_changed`、證據檔） | 把「監看最初的前景 App」用在目標就是前景的情境 |
| G9 使用者輸入狀態 | 事件前滑鼠三鍵、修飾鍵旗標、鍵碼 0–127 都必須是放開；安全輸入（secure event input）啟用時停止；輸入來源必須是預期的那一個（唯讀讀取，不切換） | headless／hidden 實驗的 HID 狀態取樣與 `keyboard_keys_clear` | 輸入來源與安全輸入的唯讀讀取；使用者活動偵測（待實驗） |
| G10 按住狀態 | 最小集合沒有長按：按下與放開在同一段同步程式內完成，中間不等待外部事件。按下後任何失敗都嘗試放開一次並記錄；這是清理，不是重試 | `VirtualSession` 的按住追蹤與 `InputsStillHeld` | 真實的放開清理與其收據；合成按鍵未放開時的實際行為（待 fixture 實驗） |
| G11 逾時 | 每個請求有總時限；前置檢查逾時時不投遞；投遞後逾時回報結果不明 | `bounded_process`、ReachAgent 連線 30 秒逾時 | 投遞路徑的時限 |
| G12 收據 | 投遞前先寫入並同步 `attempt` 紀錄，寫不進去就不投遞；之後寫 `result`。缺 `result` 一律視為送達不明 | 三個協調程式的 `attempt.json`／`result.json` 與 `trial_delivery=unknown`；0600、不覆寫既有檔 | 紀錄欄位見第 4 節 |
| G13 不重試、不換路徑 | 送達不明時停止並交給 agent 依新截圖判斷；絕不改用其他投遞路徑 | 背景輸入評估ï¼`docs/BACKGROUND-INPUT.md`ï¼的整合條件 | — |
| G14 可見意圖 | 點擊前可先用游標 overlay 顯示目標點，讓在場的使用者看到 | `overlay_session` | 投遞當下 overlay 必須已隱藏，或確認不影響 G7 |
| G15 緊急停止 | 使用者結束 ReachAgent 即停用；偵測到使用者自己動了滑鼠或鍵盤時停止（偵測方式待實驗） | ReachAgent 單連線、同使用者檢查 | 停止條件與說明 |

### 3.1 座標校準（reach#1、#6 的相依）

reach#1、#6 的實機校準還沒做（卡在憑證與權限）。點擊需要「截圖像素 → 視窗點 → 螢幕點」，這是硬相依：

- **必要**：reach#6 的 fixture 校準在同一台機器、同一個顯示器配置上完成，證明擬合與失效條件在實機成立。
- **Safari 視窗沒有標記可擬合**。建議的替代證據，全部成立才允許點擊：(a) 同一顯示器上 fixture 校準未失效；(b) Safari 擷取的圖片尺寸等於視窗框乘以回報的縮放；(c) 「移動後讀回」：先只送指標移動，再唯讀讀取游標位置，必須等於預期的螢幕點；(d) 點擊後由新的觀察確認結果，不符就停。
- 視窗移動、縮放、換顯示器或進全螢幕都讓校準失效，必須重新觀察。
- 這個替代方案是否足夠，是使用者要決定的問題（第 6 節 Q7）。更嚴格的做法是請 Surface 在測試頁畫出已知位置的標記，直接對 Safari 視窗擬合。

### 3.2 既有實驗為何停用，以及這次如何避免

- `AXPressExperiment` 的 `--run-fixture`：停用原因是焦點隔離未驗證、結果不明時沒有妥善保留收據。對應 G12、G13。
- `TargetEventExperiment` 的可見視窗入口：曾觀察到前景停用通知。前景模式不宣稱焦點隔離；它要求目標本來就在前景，並以 G8 監看。
- `HeadlessEventExperiment`、`HiddenWindowEventExperiment`：只對自己的子程序送一個 `mouseMoved`，不接受外部目標。它們的握手、HID 狀態取樣、取樣間隔門檻、`attempt`／`result` 流程是 G9、G12 的來源；程式本身維持原狀，不改成前景入口。

這些舊入口不重新啟用。前景模式是新的、範圍更窄的程式。

## 4. `cu-agent` 協定擴充

### 4.1 走 ReachAgent App，不走 helper 路徑

建議輸入**只**經 ReachAgent App：

- 事件送出權限綁在簽章身分上；走 helper 會把權限算到 terminal 或 agent 的 shell（BUILDï¼`docs/BUILD.md`ï¼的「權限與簽章」）。
- 允許清單、armed 狀態與到期時間需要一個由使用者啟動、持續存在的程序來持有；單次執行的 helper 做不到。
- `action` 不接受任何 helper 欄位。ReachAgent 沒執行或沒啟用輸入時，回報 `agent_unavailable` 或 `input_disabled`，沒有備援。

### 4.2 權限

| 權限 | 用途 | 現況 |
|---|---|---|
| 輔助使用（Accessibility） | AX 讀取；送出事件（`CGPreflightPostEventAccess` 預檢的那一項，系統設定中歸在「輔助使用」） | 尚未授與；reach#5、#6 的前置 |
| 螢幕錄製 | 擷取 | 尚未授與；同上 |
| 輸入監控（Input Monitoring） | 只有監聽他人事件才需要。本計畫**不要求** | 不要求 |

需要的協定變更：`post_event` 目前是 `not_requestable`；啟用輸入模式後才允許經 `request_permissions` 明確要求，其他情況維持現狀。`ReachAgent-Info.plist` 的事件送出說明文字在實作落地時更新。授權一律由使用者在系統設定裡自己做。

### 4.3 `action` 請求（草稿）

一次請求一個事件。欄位嚴格比對，未知欄位拒絕（沿用 `strict_json`）。

```json
{
  "id": "click-1",
  "method": "action",
  "delivery": "foreground",
  "target": {
    "bundle_id": "com.apple.Safari",
    "pid": 12345,
    "window_id": 678,
    "process_start_unix_seconds": 1762700000.25
  },
  "observation": { "image_sha256": "<64 位十六進位>" },
  "event": { "kind": "click", "button": "left", "x": 320.0, "y": 410.5 },
  "evidence_dir": "/absolute/new-directory"
}
```

`event` 的種類（第一版只有這三種）：

| `kind` | 欄位 | 意義 |
|---|---|---|
| `pointer_move` | `x`、`y` | 移到視窗內一點（點座標，視窗左上為原點），不按鍵 |
| `click` | `button`（只接受 `left`）、`x`、`y` | 同一點按下後立即放開 |
| `key_tap` | `keycode` | 按下後立即放開，無修飾鍵；鍵碼必須在啟用時給的鍵碼清單內 |

`delivery` 只接受 `foreground`；`background` 回報 `unsupported`。`observation.image_sha256` 必須是 ReachAgent 自己最近一次對該視窗擷取的那張；`key_tap` 也要求觀察，用來綁定目標與新鮮度。

### 4.4 回覆與狀態

| 狀態 | 結束碼 | 意義 |
|---|---|---|
| `unsupported` | 3 | 輸入未編入、未啟用，或 `delivery` 不是 `foreground`（維持今天的預設行為） |
| `refused` | 1 | 某個閘門拒絕，**沒有投遞**；`result.gate` 與 `result.reason` 指出是哪一個 |
| `posted_unverified` | 3 | 事件已交給系統，前後檢查都通過。**不代表 App 已處理**；agent 必須取得新觀察確認 |
| `delivery_unknown` | 1 | 可能已投遞，但事後檢查失敗、逾時或清理不完整。不得重試；保留證據 |

`result` 至少包含：`attempted`、事件前後的程序與視窗取樣、焦點監看結果、HID 狀態、輸入來源、換算後的螢幕點（只在收據裡，不回給呼叫端當座標用）、`release_attempted`／`release_confirmed`、`attempt_file`、`result_file`、各階段的單調時間。回覆頂層的 `action_allowed` 維持現有語意：它描述「這個回覆是否授權下一個動作」，仍然恆為 `false`。

`describe` 新增 `input` 區塊：`state`（`disabled`／`armed`）、允許的 bundle id 與鍵碼、到期時間、剩餘事件數。`capabilities` 的 `foreground` 與 `keyboard` 只在實機驗證（第 5 節 I-4、I-5）完成後才改寫，並註明範圍。

### 4.5 相關的唯讀新增

重測要讀回的證據需要幾個唯讀能力，與輸入分開做、分開驗：目前輸入來源、安全輸入狀態、指標位置讀回、依 bundle id 找輸入法程序的視窗（候選窗）。它們不送事件。

### 4.6 離線可測的部分

`simulate_input`、`preview_native_input` 與 `VirtualSession` 繼續當離線排練。`input_guard` 需要一個前景變體：目前 `Delivery::Foreground` 一律回 `ForegroundUnsupported`、`Destination::Desktop` 一律拒絕；前者在新模式下改為需要額外前置條件，後者維持拒絕。ReachAgent 的輸入後端比照擷取與無障礙，分成假後端（`--self-test`、CI）與系統後端；CI 只跑假後端，並加一個靜態檢查：呼叫事件投遞 API 的原始碼只能出現在那一個後端檔案。

## 5. 分階段的 issue 拆分（草稿）

相依順序：I-1 → I-2 → I-4 → I-5 → I-6；I-3 只依賴 reach#5、#6 的權限，可與 I-1、I-2 並行。reach#5、#6（憑證、授權、實機擷取與校準）是 I-3 之後所有實機步驟的前置。

### I-1 前景輸入的請求、閘門與收據：離線設計與測試

- 範圍：`action` 的 event schema（4.3、4.4）；`input_guard` 的前景變體；允許清單、鍵碼清單、到期與計數的純資料模型；`attempt`／`result` 紀錄格式；假後端。只有 Rust，離線。
- 驗收：
  - [ ] 三種事件、四種狀態、每個閘門的拒絕都有單元測試；未知欄位、背景 `delivery`、不在清單內的 bundle id 或鍵碼一律拒絕。
  - [ ] 假後端測試涵蓋：按下後失敗會嘗試放開、`attempt` 寫不進去就不投遞、缺 `result` 視為送達不明、不重試。
  - [ ] `cu-agent` 的 `action` 在預設狀態仍回報 `unsupported`，既有測試不變。
  - [ ] 沒有任何程式碼呼叫事件投遞 API。
- 使用者在場：不需要。

### I-2 ReachAgent 輸入後端骨架與啟用機制

- 範圍：ReachAgent 的 `--enable-input`、`--allow-bundle`、到期與上限；`post_event` 權限的預檢與明確要求；`describe` 的 `input` 區塊；假後端與系統後端的分離；`cu-agent` 的 `action` 路由（只走 ReachAgent）。系統後端編入但在 `--test-mode` 與 CI 不可達。
- 驗收：
  - [ ] `--self-test` 以假後端涵蓋啟用、到期、清單、拒絕與收據；不需要權限、不送事件。
  - [ ] 靜態測試：事件投遞 API 只出現在系統後端檔案。
  - [ ] 未帶 `--enable-input` 時行為與今天完全相同。
  - [ ] 文件說明啟動方式與限制；README 等「輸入停用」的敘述只在這個 PR 合併、事實改變時才更新，並寫清楚預設仍停用。
- 使用者在場：不需要（不送事件）。相依：I-1。

### I-3 重測所需的唯讀觀察（Safari、注音）

- 範圍：4.5 的唯讀能力；2.4 第 1 項的實機唯讀觀察。使用者自己打字，reach 只讀。
- 驗收：
  - [ ] 能唯讀讀到目前輸入來源與安全輸入狀態。
  - [ ] 記錄候選窗屬於哪個程序、能否取得其視窗邊界；讀不到就如實記錄並停止。
  - [ ] 記錄 Safari 中 `#status` 文字與隱藏 textarea 的框能否經 AX 讀到，並與 Surface 的需求（S2、S3）對照。
  - [ ] 不送任何事件；證據只含尺寸、雜湊與摘要，不上傳截圖。
- 使用者在場：**需要**（授權已完成、親手打字）。相依：reach#5、#6 的憑證與權限。

### I-4 自建 fixture 的前景指標輸入：實機驗證

- 範圍：新增 fixture App（已知位置的目標，記錄收到的事件、座標與時間）；對它做 `pointer_move`、讀回、`click`。只允許 fixture 的 bundle id。
- 驗收：
  - [ ] fixture 收據與請求的點一致（容許誤差須記錄）；移動後讀回的游標位置等於預期螢幕點。
  - [ ] 涵蓋負面情境且確認**沒有投遞**：目標不在前景、被遮住、視窗移動後校準失效、身分不符、觀察過期、清單外的 bundle id。
  - [ ] 涵蓋結果不明：投遞後終止 ReachAgent，確認留有 `attempt`、沒有重試、滑鼠鍵狀態為放開。
  - [ ] 記錄對使用者的影響（游標被移到哪裡、前景是否改變），不宣稱隔離。
  - [ ] 可重現的命令與證據摘要寫進文件。
- 使用者在場：**需要**（第一次真實事件；約定時段，期間不碰鍵鼠）。相依：I-1、I-2、reach#6 的實機校準。

### I-5 自建 fixture 的鍵盤與注音輸入：實機驗證

- 範圍：fixture 加文字欄位，記錄 `keyDown`、marked text、送出的文字。送 1.3 的鍵碼。
- 驗收：
  - [ ] 在一般英文輸入來源下，`key_tap` 送達且字元正確。
  - [ ] 在注音輸入來源下，鍵碼 18 產生 ㄅ 的組字；23、38、44 產生 ㄓㄨㄥ；空白鍵兩次後候選窗出現，並由 I-3 的唯讀方法量到邊界。
  - [ ] 若組字或候選窗沒有出現，如實記錄並停止；不改用其他投遞方式。
  - [ ] 涵蓋：輸入來源不是預期時拒絕、修飾鍵旗標非零時拒絕、安全輸入啟用時拒絕、按下後失敗的放開清理。
  - [ ] 殘留組字的清理步驟有紀錄。
- 使用者在場：**需要**（由使用者先選好注音；reach 不切換輸入來源）。相依：I-3、I-4。

### I-6 對 Safari 的受控啟用與 surface #96、#97 重測

- 範圍：把 `com.apple.Safari`（與經核准的 Chrome）加入允許清單的程序與文件；G4 的網址檢查；依 Surface 確認的序列執行重測並交付證據。
- 驗收：
  - [ ] Surface 的 S2–S6、S8 都有答案並記錄（S1、S7 已答）。
  - [ ] 每一步都有 `attempt`／`result` 與事後觀察；任何 `refused` 或 `delivery_unknown` 就停止，不補送。
  - [ ] 交給 Surface 的證據包含它驗收要的欄位（瀏覽器、OS、輸入來源、`isTrusted`、計數與焦點、caret 矩形與候選窗邊界），並註明每一項的取得方式與限制。
  - [ ] 重測結論由 Surface 判定；reach 只提供工具與證據，不含 surface 專用邏輯。
  - [ ] 結束後 ReachAgent 回到停用狀態。
- 使用者在場：**需要**（全程；Safari 視窗、分頁與輸入來源由使用者備妥）。相依：I-3、I-4、I-5，以及 Surface 的確認。

### 後續（不在這次範圍）

拖曳與長按、右鍵與中鍵、捲動、修飾鍵組合、文字直接輸入、AX 動作、由 reach 啟動或切換 App、由 reach 切換輸入來源、背景輸入（reach#2）、MCP 包裝下的輸入（reach#3）、其他 App 的允許清單、隔離工作階段內執行。

## 6. 需要使用者決定的問題

| # | 問題 | 建議 |
|---|---|---|
| Q1 | 是否接受這次用**前景**模式：會移動實體游標、Safari 必須在前景、執行期間使用者不能碰鍵鼠？ | 接受。這是 #96、#97 語意上的必要條件；只在有人在場的約定時段執行。 |
| Q2 | reach#2 怎麼處理？ | 保持開啟與 `blocked`，範圍仍是背景輸入。前景模式另開 I-1 至 I-6，並在其中記錄 2026-11-10 的指示與「只限 surface #96、#97」的範圍。 |
| Q3 | 誰把 Safari 帶到前景、選注音、載入與重新載入頁面？ | 第一版由在場的使用者手動做，reach 只唯讀驗證。reach 不新增啟動 App 或切換輸入來源的能力。 |
| Q4 | 允許清單放哪些 bundle id？Surface 已確認 #97 要在 Safari 之後重複於 Chrome，是否同意把 Chrome 也列入？ | fixture、`com.apple.Safari`，以及 Safari 重測完成後以同樣程序加入的 `com.google.Chrome`。Surface 的確認不等於使用者核准；Chrome 需要使用者明說同意。 |
| Q5 | 憑證與授權何時做？（建立本機簽章憑證、安裝 ReachAgent、授與輔助使用與螢幕錄製；輸入另需同意事件送出） | 先做，因為它同時解開 reach#5、#6。全部由使用者親手操作。 |
| Q6 | 真實事件實驗（I-4、I-5、I-6）的時段與條件？ | 每次約定一個時段；開始前關閉不相關的視窗，執行期間不碰鍵鼠；每次啟用設 10 分鐘到期與事件數上限。 |
| Q7 | Safari 視窗的座標證據，是否接受 3.1 的替代方案（fixture 校準＋幾何檢查＋移動後讀回＋事後觀察）？ | 接受作為第一版；若實測誤差不可接受，再請 Surface 在測試頁提供標記。 |
| Q8 | 是否請 Surface 把 `isTrusted`（以及必要時 caret 矩形）顯示在畫面上（S2、S3）？Surface 目前的說法是靠 console 或 debug 讀數，reach 讀不到 console。 | 是，並由 Surface 開自己的 issue。控制項位置（S1）已定案為由擷取或 OCR 判讀，不需要頁面輸出。reach 不執行頁面 JavaScript，也不放 surface 專用邏輯。 |
| Q9 | 重測用哪個 Safari 環境？ | 專用的 Safari 視窗，只開 showcase 一個分頁，網址限本機 dev server；不在有其他登入分頁的視窗上執行。 |
| Q10 | 「綠色按鈕」步驟（S4）若是全螢幕，要不要納入第一輪？ | 第一輪先做不需要它的驗收項目；確認 S4 後再決定。若需要 Option＋點擊，屬於修飾鍵，另行核准。 |
| Q11 | I-1、I-2（離線、不送事件）是否可以在其他問題定案前先開工？ | 可以。它們不改變預設行為，也不需要權限。 |
