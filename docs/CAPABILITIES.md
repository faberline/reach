# 通用 Rust Computer Use 框架

此專案是通用層。它管理 macOS 視窗身分、座標、截圖、無障礙資料和輸入後端。它不包含任何特定遊戲的畫面類別、隊伍或遊戲規則。

Agent 可使用[通用 JSON CLI](AGENT-CLI.md)逐步呼叫視窗、無障礙、擷取及未來輸入介面。`action` 請求預設回報不支援，不送出事件；前景輸入只在 ReachAgent 以 `--enable-input` 啟動時經由它處理，見[服務協定](AGENT-SERVICE.md)。 單獨建置方式見[獨立建置說明](BUILD.md)。視窗身分、擷取證據欄位與限制見[視窗身分與擷取證據](WINDOW-IDENTITY.md)。座標校準擬合與失效條件見[座標校準](CALIBRATION.md)。

遊戲畫面判讀、Core ML、關卡決策和紀錄由呼叫端的獨立程式負責，不在 Reach。該程式透過同一個 JSON CLI 或 Rust library 呼叫 Reach。

## 現況

- `windows --pid PID` 列出指定程序的可見視窗候選、Core Graphics 原始 layer、可取得的標題及表面分類。layer 為 0 顯示 `normal_layer_candidate`，其他數值顯示 `non_normal_layer`，缺失或無法讀取顯示 `unknown_layer`。標題缺失、錯誤或過長時為 `null`；標題不是唯一身分。layer 0 也不能證明這是一般 App 視窗、沒有遮擋或擁有焦點。清單不授權操作。
- `capture --pid PID --window ID --output /absolute/new.png` 在擷取前後檢查 PID、程序啟動時間、視窗 ID、layer 和幾何，並檢查擷取器與視窗清單的座標是否相同。未知或非 0 layer 不進行擷取；擷取器另要求 ScreenCaptureKit 排除桌面視窗。這些檢查不證明 layer 0 是安全操作目標。它解碼 PNG，核對實際像素尺寸是否符合擷取資料，並保存 SHA-256 和同名 `.json` 證據；證據不覆寫既有檔案。擷取失敗時會記錄原因，並將可能存在的圖片標為歸屬未確認。這些檢查不證明圖片座標與視窗座標已校準。畫面預設是 `unknown`，不允許操作。
- `action` 預設明確回報 `unsupported`。前景 `action`（`delivery: "foreground"`）只轉給以 `--enable-input` 啟動的 ReachAgent；App 不在時回 `agent_unavailable`，未啟用時回 `unsupported`（`input_disabled`）。系統輸入後端尚未在真機執行或驗證，不搶使用者滑鼠及焦點的背景輸入也尚未完成。
- ReachAgent 的唯讀觀測（I-3）：`input_source`（目前的鍵盤輸入來源）、`secure_input`（安全輸入是否開啟）、`pointer_location`（指標位置）與 `bundle_windows`（某 bundle id 在螢幕上的視窗與 layer、bounds）。只讀狀態，不需要權限、不彈窗、不切換輸入來源、不移動指標、不讀標題；經 `cu-agent` 與 `cu-mcp` 只能透過 ReachAgent 呼叫。**實機未驗證**，前景輸入的閘門也不使用這些讀值（G7 至 G9 仍失敗即關閉），接上要等 I-4 實機驗證。見[服務協定](AGENT-SERVICE.md)。
- `capabilities` 以 JSON 回報目前的靜態實作範圍。它不檢查權限或目標，也不啟動 helper。背景輸入、前景輸入、鍵盤與 AX 動作都回報不支援；游標與焦點隔離仍未驗證。
- `display-inventory` 是獨立的唯讀程式。它連續讀取兩次 Core Graphics 的活動顯示器、位置、尺寸、像素數、旋轉和鏡像來源；兩次不一致或資料不完整時回報 `unknown`。相同也只代表兩次取樣一致，不代表兩次之間沒有變化。它不驗證截圖像素與畫面座標的對應，不允許輸入。建置及執行：`CARGO_HOME=/private/tmp/sw-cargo-cache cargo run --locked --offline --bin display-inventory`。
- `input_guard` 可離線檢查視窗、座標、畫面期限與身分是否相符。資料由呼叫端提供；此檢查不證明資料來源，也不允許輸入。
- `accessibility --pid PID` 唯讀列出指定程序的無障礙視窗和受限樹，包含角色、標題、座標及可用動作。文字若超過限制會標記 `role_truncated`、`title_truncated`，或在 `action_names_truncated_indices` 列出受影響的動作索引；全域 `truncated` 也會設為 true。不可把截斷的文字當成完整識別。它不送出動作；AX 視窗和 Core Graphics 視窗的關聯尚未確認。權限不足或程序身分變動會停止。
- `observe-binding --pid PID --window ID` 唯讀收集程序身分、前後 CG 清單和 AX 視窗摘要，再呼叫 AX／CG 比對器。結果包含缺少的證據，供 Agent 判斷。畫面配置和座標校準尚未驗證，所以目前一定回報 `unknown`、`action_allowed: false`，並以狀態碼 4 結束。可將 JSON 輸出保存為證據；其中可能有視窗標題及程序路徑。它不執行操作。
- Rust library 的 `ax_cg::match_candidate` 是純資料比對器，不呼叫系統或操作畫面。它要求呼叫端提供前後完整 CG 視窗清單（含所有程序且由前到後排序）、AX 視窗快照、程序身分、顯示器摘要與座標校準資料。缺資料、清單或身分變動、視窗重疊及多個匹配會拒絕。唯一匹配只回傳 `CandidateUnverified`，所有結果的 `action_allowed()` 都是 `false`。這些輸入都是呼叫端的陳述；比對器不驗證來源，也不證明校準、遮擋、瞬間變化或視窗綁定正確。
- 一般 App 的判讀介面仍在建立。視窗 ID 不保證跨程序重啟仍有效；每次要重新查證。視窗清單也不能證明沒有其他視窗遮住。

## 建置與執行

需要 macOS、Rust、Swift、Cargo 相依套件快取，以及系統螢幕錄製與輔助使用權限。擷取與無障礙 helper 都在此專案內建置，不依賴遊戲專案的執行檔。

```sh
# 在儲存庫根目錄執行
sh build-capture.sh
sh build-ax.sh
CARGO_HOME=/private/tmp/sw-cargo-cache cargo test --locked --offline
CARGO_HOME=/private/tmp/sw-cargo-cache cargo build --locked --offline
./target/debug/mac-computer-use windows --pid PID
./target/debug/mac-computer-use capabilities
./target/debug/mac-computer-use accessibility --pid PID
./target/debug/mac-computer-use observe-binding --pid PID --window WINDOW_ID
./target/debug/mac-computer-use capture --pid PID --window WINDOW_ID --output /absolute/new.png
./target/debug/mac-computer-use action
```

PID 和窗口 ID 是即時資料，勿沿用舊值。輸出檔必須不存在。`action` 只回報不支援，不發送輸入。

## 驗證

2026-11-08：離線測試確認程序身分或視窗幾何改變時停止，並拒絕覆寫證據。獨立版本透過遊戲專用程式成功唯讀擷取 iPad Sidecar 的 2204×1484 圖片，並保存擷取身分、程序啟動時間及 SHA-256。此單次測試不證明所有視窗或環境都可用。

同日加入 layer 檢查後，再透過遊戲專用程式唯讀擷取 1814×1424 的遊戲商店畫面。目標 layer 為 0；Rust 與 Core ML 均棄權，程式保存圖片及覆核證據後以狀態碼 3 停止。這不證明圖片未被遮擋，也不驗證輸入。擷取畫面（原始證據：來源專案 `screenshots/rust-bot-20261108-layer-verified.png`，未隨本庫攜帶）

同日以獨立版本唯讀列出遊戲程序的三個視窗候選，其中主視窗為 1102×742 點。一般沙箱內無法連接視窗伺服器；授權後的程序可列出資料。此檢查沒有擷取畫面或輸入，也不能證明視窗未被遮擋。

同日新版清單唯讀列出 Finder 程序 877、視窗 4763，回傳 layer `-2147483603`、分類 `non_normal_layer` 和空標題；此數值只表示當時資訊，不能推定其他 Finder 視窗種類。針對該目標的擷取請求在程序身分檢查就失敗，沒有產生圖片；這次並未實機證明 layer 拒絕邏輯。未知及非 0 layer 在離線假後端測試均不會呼叫擷取器。排除桌面視窗後的遊戲擷取尚未實機重測。

同日以自建測試 App 驗證 AX：讀到一個視窗和按鈕，測試前後查到的前景仍是 ChatGPT。AppKit 將原本預計畫面外的測試視窗移進螢幕範圍；fixture 已改為不顯示視窗，重測時前景未切換，AX 回報零個可讀視窗。這不是對其他 App 的準確度或輸入能力驗證。

### AX／CG 候選比對的接入限制

`Backend::windows(None)` 提供按前後順序的全程序螢幕視窗資訊；一般清單 CLI 只列指定 PID。AX helper 現在輸出經 AX 核對的節點 `owner_pid` 和程序可執行檔路徑。`observe-binding` 按程序身分、顯示器、CG、AX、CG、顯示器、程序身分的順序收集。輸出只包含目標程序的 AX 視窗與指定 CG 視窗，不列出其他程序的視窗標題。

目前沒有可供此比對器驗證的顯示器配置摘要、座標校準記錄，也不能獨立證明 CG 清單完整。新觀察流程不假造這些證據。比對器目前使用 3 秒掃描和 60 秒校準時限；這些是未經實機校準的保守限制，不是安全保證。此流程尚未驗證視窗對應，也沒有接入任何輸入。Finder 曾因程序啟動身分不可用而無法取得 AX 快照；不能據此推測視窗對應。

目前比對器因沒有座標校準資料而立即停止。它還沒有對真實 AX 與 CG 視窗執行配對。主要 `reason` 因此是 `calibration_missing`，不能當成其他欄位皆已通過的證據。

`evidence.total_elapsed_ms` 用單調時鐘量測，包含前後兩次程序身分讀取。原有的 `elapsed_ms` 只涵蓋 CG／AX 掃描。`started_unix_ms` 和 `completed_unix_ms` 是系統時間，可能被調整，不能單獨證明資料新鮮。證據保留可取得的程序 PID、啟動時間、執行檔路徑和 bundle ID；`null` 代表當次沒有有效身分資料。中途失敗時，也會回報已取得的程序身分和耗時。這些資料不授權輸入。

2026-11-08 對遊戲視窗做唯讀觀察時，AX 和 CG 的目標視窗標題與幾何一致。因清單完整性、顯示器配置和座標校準仍未確認，結果是 `unknown`，不可操作。原始證據：來源專案 `screenshots/rust-ax-cg-binding-20261108.json`（未隨本庫攜帶）。這次觀察沒有送出輸入，也不能證明輸入不會影響游標或焦點。

同日 `display-inventory` 在一般沙箱中無法取得活動顯示器，回報 `unknown`；在授權的唯讀程序中取得兩個活動顯示器，兩次取樣一致。當時的紀錄（原始證據：來源專案 `screenshots/rust-display-inventory-20261108.json`，未隨本庫攜帶）只含活動顯示器，可能不含硬體鏡像中其他已連線的螢幕。

離線原型讓 `observe-binding` 在視窗及 AX 觀察的前後讀取活動顯示器。輸出保存兩次結果、取樣時間，以及已取得的目標視窗和 AX 摘要。未能取得資料時會停止；若盤點內部的第二次讀取失敗，盤點器無法返回第一次的資料，結果會標明這項證據遺失。兩次不同或資料不完整會列入 findings。兩次相同也無法排除中間變動，不能當成座標校準或輸入驗證。取樣沒有強制逾時；時間紀錄不保證資料新鮮。結果仍是 `unknown`、`action_allowed: false`。這個原型尚未接入截圖流程。2026-11-08 對遊戲做了一次唯讀測試，兩次顯示器取樣一致，結果仍為 unknown，禁止輸入。觀察紀錄（原始證據：來源專案 `screenshots/rust-ax-cg-display-binding-20261108.json`，未隨本庫攜帶）保留視窗與顯示器摘要；它不證明清單完整或座標已校準。

## 背景輸入研究

[trycua/cua-driver](https://github.com/trycua/cua/blob/main/libs/cua-driver/README.md) 是另一個專案。其核心驅動採 MIT 授權，並聲稱支援背景視窗輸入。它的 [macOS 說明](https://github.com/trycua/cua/blob/main/libs/cua-driver/rust/Skills/cua-driver/MACOS.md) 指出，此方式使用非公開的 SkyLight 介面，部分遊戲可能忽略背景事件或讀取實體游標位置。此專案尚未安裝、整合或驗證該驅動。未來若評估，先在受控測試 App 驗證游標、焦點、座標及操作結果；測試 App 成功不代表遊戲也能使用。不得使用會移動實體游標的 desktop 目標或會切換焦點的 foreground 模式。

### 舊版可見視窗實驗已停用

`sh build-ax-press-fixture.sh` 與 `sh build-target-event-fixture.sh` 只編譯並執行離線自測。AXPress 的 `--run-fixture`，以及程序定向測試的可見視窗入口 `--run-fixture`、`--observe-fixture`、`--observe-passive-fixture` 均停止，不啟動子程序或發送輸入。先前可見視窗測試曾觀察到前景停用通知；AXPress 原型也未妥善保留結果不明時的收據。這些入口必須重新設計並驗證焦點隔離和證據保存後，才能恢復。

程序定向測試的 `--observe-hidden-fixture` 仍是明確啟動的唯讀診斷。它不授權任何輸入，也不能證明可見視窗或其他 App 的焦點隔離。所有程式的 `action` 仍停用。

