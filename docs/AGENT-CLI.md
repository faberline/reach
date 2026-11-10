# 通用 Computer Use 的 JSON CLI

本介面不屬於任何特定 agent；任何能啟動程序並讀寫 JSON 的程式都能使用。文中 `<REACH_DIR>` 代表本儲存庫在呼叫者環境中的絕對路徑；helper 路徑與輸出路徑必須使用呼叫者目前環境中的絕對路徑。

`cu-agent` 接收一筆 JSON，執行一次呼叫，再輸出一筆 JSON。它可供 Agent 直接使用，或經由 [MCP stdio 包裝](MCP.md) 使用；`cu-agent` 本身不是 MCP server。它支援唯讀觀察與保存擷取證據，不發送輸入。

```sh
cd <REACH_DIR>
CARGO_HOME=/private/tmp/sw-cargo-cache cargo build --locked --offline --bin cu-agent
printf '%s\n' '{"id":"discover-1","method":"describe"}' | target/debug/cu-agent
printf '%s\n' '{"id":"windows-1","method":"windows","pid":12345}' | target/debug/cu-agent
```

stdin 接收一筆 UTF-8 JSON；送出後須關閉 stdin。大小上限 65,536 bytes。`id` 是非空字串。未知、缺少或重複的欄位會拒絕。巢狀物件也不可有重複欄位。`describe` 回報完整方法與欄位。

- `windows`：依 PID 列出視窗候選。視窗 ID 與名稱不足以證明目標正確。
- `all_windows`：依 PID 列出包含非螢幕視窗的 Core Graphics 候選；範例 `{"id":"all-1","method":"all_windows","pid":12345}`。無效的候選視窗幾何或識別資料會被排除，並以 `omitted_invalid_count` 回報。清單可能不完整或過期，PID 可能重用；此呼叫不檢查程序身分、畫面、可見性或擷取資格。結果一律為 `unknown`、狀態碼 3、`action_allowed=false`。原本的擷取與輸入限制不變。
- `observe_all_windows`：接受 `pid` 和可信的絕對路徑 `ax_helper`。列出 `all_windows` 前後各取樣兩次程序身分，並保留兩組取樣和視窗清單。初始兩次身分不同時不列舉；列舉或最後查詢失敗時回報原因，已取得的候選保留。`same_identity_samples=true` 只表示四次身分欄位一致，不代表視窗綁定或期間沒有變化。省略數量不為零時清單也不完整。結果仍是 `unknown`、狀態碼 3、`action_allowed=false`；呼叫端須自行保存回覆作證據。
- `record_all_windows`：接受 `pid`、可信的絕對路徑 `ax_helper` 和新檔絕對路徑 `output`。先以權限 0600 建立新檔，再執行 `observe_all_windows`；既有檔案不覆寫。結果與初始查詢失敗原因會寫入並同步檔案，可能包含程序路徑、視窗標題和座標。身分取樣不一致、查詢失敗或有被排除的候選時回報 `observation_incomplete`、狀態碼 1；其他情況回報 `unknown`、狀態碼 3。`observation_complete=true` 只表示這些有限檢查完成，不代表清單完整、畫面新鮮、視窗綁定或可輸入。程序中斷或寫入失敗可能留下空檔或部分檔案；不應視為成功。目錄內容與 helper 仍須由呼叫端信任。
- `find_windows`：以完整標題搜尋目前螢幕上的視窗候選，回傳 PID 和視窗 ID。無標題視窗不會列出；標題可能重複或變動。結果一律為 `unknown`，須重新核對程序身分和視窗。
- `find_bundle`：依 `bundle_id` 和 `ax_helper` 兩次查找執行中的 App。結果列出所有候選與資料不完整的數量。兩次一致不保證資料完整或持續有效。此功能需要重新執行 `sh build-ax.sh`。
- `find_bundle_windows`：在查找 App 前後取樣，列出候選程序的螢幕視窗。取樣不一致、程序過多或視窗讀取失敗時停止並回報原因。結果一律為 `unknown`，不能代表視窗綁定或授權輸入。 候選程序可能包含 `app_bundle_path`；它是系統回報的 App 路徑，可能為 null、可能在下次啟動時改變，也不證明路徑和目標視窗的綁定。舊 helper 可能不含此欄位。
- `process_identity`：依 PID 和 `ax_helper` 連續讀取兩次程序啟動時間、執行檔與 bundle ID。若第二次失敗或資料無效，保留第一次的有效結果，回報 `same_identity_samples=false` 和原因；第二次的無效內容不保留。兩次一致仍不保證中間沒有變化，也不授權輸入。
- `accessibility`：依 PID 和絕對路徑 `ax_helper` 取得無障礙資訊。

  若 App 回報的 `AXWindows` 清單為空，完整樹查詢會額外讀取主要視窗和焦點視窗，將不同元素列於 `fallback.candidates`；同一元素會合併來源。`fallback.scope=main_or_focused_incomplete`、`complete_window_list=false`，它不取代原本的空清單，也不證明對應哪一個 Core Graphics 視窗。候選樹可能因節點、深度或時間上限截斷；須檢查頂層 `truncated`。僅頂層視窗查詢不使用此備援。結果不授權操作；使用前須重新執行 `sh build-ax.sh`。
- `accessibility_subtree`：接受 `pid`、絕對路徑 `ax_helper`、`source`（`main`、`focused` 或 `windows`）和 `path`（最多 8 個 0 至 159 的索引）。例如 `{"id":"branch","method":"accessibility_subtree","pid":12345,"ax_helper":"/absolute/mac-ax-snapshot","source":"main","path":[0,1]}`。從 `fallback.candidates` 內的節點 `path` 移除第一個候選索引，再使用該候選的 `sources` 中一個來源。若來源為 `windows`，直接使用一般視窗樹的 `path`；第一個索引必須是 0 至 31 的視窗索引，不可省略。每次均重新讀取，路徑可能指向不同元素；不可用它操作或假定與舊截圖綁定。無效路徑、跨程序元素或來源不可讀時回報錯誤。成功仍為 `unknown`、狀態碼 3、`action_allowed=false`，須檢查 `truncated`。使用前須重新建置 AX helper 和 CLI。
- `observe_binding`：依 PID、`window_id` 和 `ax_helper` 取得視窗及顯示器比對證據，結果仍為 `unknown`。若要留存證據，呼叫端須保存回覆。
- `observe_window`：一次收集截圖與 AX／CG 證據；詳見 [單次視窗觀察](JOINT-OBSERVATION.md)。
- `capture`：依 PID、`window_id`、新的 `image` 路徑，以及 `capture_helper`、`ax_helper` 的絕對路徑，保存圖片與 sidecar。成功擷取仍回報 `unknown`，不能據此操作。
- `capture_all_windows`：與 `capture` 相同欄位，以明確的 `--include-offscreen` helper 模式擷取。須先執行 `sh build-capture.sh` 取得支援該模式的 helper。macOS 後端依目標視窗編號單獨查詢，擷取前後核對程序身分、視窗擁有者、層級與幾何。其他視窗的無效資料不會因此阻止目標查詢；目標缺失、重複或資料無效時停止。其他後端若沒有實作單一查詢，預設仍會在整份清單有排除項目時停止。sidecar 的 `inventory_scope=exact_window_candidate` 標明範圍，成功仍回報 `unknown`、`action_allowed=false`。這些是非原子取樣；圖片內容、時效、視窗完整性和輸入安全均未獲證明。失敗後圖片可能保留，所有權未確認；須查看 sidecar，不要自動重試。

`capture_all_windows` 的擷取紀錄包含 ScreenCaptureKit 的 `onscreen_before` 與 `onscreen_after`；helper 發現狀態改變時停止，Rust 也拒絕缺少、不一致或非布林的狀態。兩次一致不表示畫面未遮擋或取樣之間未變，也不能證明截圖是最新的。舊版 helper 不含欄位，須重新建置。
- `recognize_text`：以可信 OCR helper 讀取已保存的圖片；詳見 [OCR 說明](OCR.md)。結果始終需 Agent 核對，不能授權操作。
- `recognize_image`：以可信本機 `adapter` 讀取已保存的 PNG。adapter 接收圖片路徑，輸出 JSON：`status` 必須為 `review_required`，`action_allowed` 必須為 `false`，並包含實際 `image_sha256`、`image_width` 和 `image_height`。其餘欄位由辨識器定義。程式核對前後圖片、雜湊及尺寸；辨識內容仍需 Agent 複核。此介面可供未來 Core ML adapter 使用，目前沒有自動接入模型。
- `interpret_image`：對同一張已保存的 PNG 依序執行 1 到 4 個可信本機 adapter，並把每個結果寫入新的證據檔。欄位為 `image`、`adapters`（1 到 4 個不重複的絕對路徑）與 `output`（新檔的絕對路徑）。證據檔權限為 0600，既有檔案不會被覆寫。adapter 的輸出除了 `recognize_image` 要求的欄位，還必須包含 `interpreter.name`、`interpreter.version`，以及 `answer`（字串，或 `null` 表示棄權）；其他欄位（分數、框、類別）原樣保留在 `raw`。每筆紀錄的 `outcome` 為 `answered`、`abstained`、`invalid_format` 或 `failed`；`agreement` 為 `agree`、`single`、`disagree` 或 `incomplete`。全部 adapter 回答或棄權、且圖片前後雜湊相同時，回報 `unknown`、結束碼 3（包含 `disagree`）；有 adapter 失敗、逾時（每個 adapter 30 秒）、格式無效或圖片改變時，回報 `interpretation_incomplete`、結束碼 1。圖片無效、`output` 已存在或請求欄位無效時，回報 `error`、結束碼 1。答案、分數、OCR 框、類別和判讀器一致都不是操作授權，`action_allowed` 一律為 `false`。adapter 是呼叫端信任的執行檔，`adapter_sha256` 只描述讀取當下的檔案內容。adapter 檔案超過讀取大小上限時，`adapter_sha256` 為 `null`，原因記錄在 `adapter_sha256_error`。現有 OCR helper 不輸出 `interpreter` 與 `answer`，用於 `interpret_image` 不會得到有效判讀，這是預期行為。程序中斷可能留下空檔或部分檔案。範例（路徑為佔位符）：

  ```sh
  printf '%s\n' '{"id":"interpret-1","method":"interpret_image","image":"/absolute/existing.png","adapters":["/absolute/adapter-a","/absolute/adapter-b"],"output":"/absolute/new-interpretation.json"}' | target/debug/cu-agent
  ```
- `overlay_session`：經由可信的 `overlay_helper` 在一次呼叫期間顯示游標 overlay，依序執行 `steps`；呼叫結束、逾時、helper 終止、回覆無效或螢幕配置改變時一定停止 helper，overlay 隨之消失。只顯示，不送任何輸入事件；顯示不等於授權輸入。座標未綁定視窗。`action_allowed` 一律為 `false`。
  - 欄位：`overlay_helper`（可信的絕對路徑，通常是 `target/debug/mac-cursor-overlay`，先執行 `sh build-cursor-overlay.sh`）、`timeout_ms`（100 到 60000 的整數，限制所有步驟的總時間）、`steps`（1 到 64 個物件）。
  - 六種 step 與它們的鍵：`show`、`move`（`command`、`display_id`、`x`、`y`）；`phase`（`command`、`phase`）；`hide`、`query`（只有 `command`）；`wait`（`command`、`ms`，1 到 10000）。`quit` 不是 step，`cu-agent` 最後一定會送。
  - overlay 只在這次呼叫期間存在；呼叫結束時一定清除。
  - 結果欄位：`completed`、`stop`（`step`、`reason`、`helper_reason`）、`steps`、`last_reported_state`、`cleanup`、`overlay_after`。
  - `stop.reason` 的值：`helper_unavailable`、`helper_exited`、`timeout`、`invalid_reply`、`helper_refused`、`unsolicited_reply`（螢幕配置改變時 `helper_reason` 是 `display_changed`）。
  - 狀態：全部步驟完成時回報 `unknown`、結束碼 3；中途停止時回報 `overlay_incomplete`、結束碼 1，而且 `last_reported_state` 是 `"unknown"`，不沿用先前的狀態；請求欄位無效時回報 `error`、結束碼 1。
  - 範例（路徑是佔位符）：

    ```sh
    printf '%s\n' '{"id":"overlay-1","method":"overlay_session","overlay_helper":"/absolute/mac-cursor-overlay","timeout_ms":5000,"steps":[{"command":"show","display_id":1,"x":100,"y":100},{"command":"phase","phase":"hold"},{"command":"wait","ms":500},{"command":"query"},{"command":"hide"}]}' | target/debug/cu-agent
    ```
- `preview_native_input`：先檢查虛構輸入序列，再建立並釋放記憶體事件；不投遞事件。
- `preview_tagged_input`：在記憶體事件加入虛構視窗編號，並讀回兩個欄位；只支援指標事件，不投遞事件。詳見 [虛擬輸入說明](VIRTUAL-INPUT.md)。
- `simulate_input`：驗證虛構的輸入序列；詳見 [虛擬輸入說明](VIRTUAL-INPUT.md)。不讀畫面、不投遞事件。
- `sample_focus_notifications`：以可信 `notification_helper` 觀察最初前景 App 的焦點與停用通知約一秒。回報註冊是否成功及通知數量。只做唯讀取樣；未收到通知不能證明焦點未變，也不授權輸入。先執行 `sh build-focus-notifications.sh` 建置 helper。
- `session_status`：透過可信 `session_helper` 唯讀取得目前登入工作階段的 console 和登入狀態；詳見 [隔離工作階段](ISOLATED-SESSION.md)。結果不能識別遠端工作階段或證明輸入隔離。
- `sample_isolation`：唯讀取樣使用者環境；詳見 [隔離取樣說明](ISOLATION-OBSERVATION.md)。結果不證明輸入安全。
- `action`：可傳入含非空 `kind` 的 `event` 物件，以取得明確的 `unsupported`。事件內容的 schema 尚未定義，也不經驗證。此呼叫不發送任何滑鼠、鍵盤或其他事件。

擷取範例：先確認本次的 PID 和視窗 ID，再替換 `12345`、`67890` 與 PNG 路徑。先依主 README 建置 helper。

```sh
cat <<'JSON' | target/debug/cu-agent
{
  "id": "capture-1",
  "method": "capture",
  "pid": 12345,
  "window_id": 67890,
  "image": "/private/tmp/new-frame.png",
  "capture_helper": "<REACH_DIR>/target/debug/mac-window-capture",
  "ax_helper": "<REACH_DIR>/target/debug/mac-ax-snapshot"
}
JSON
```

同一路徑的 PNG 和 sidecar 不可已存在。回覆的 `result.evidence_file` 指向 sidecar；若擷取失敗，紀錄會說明原因。若無法保存紀錄，回覆會報錯，不應假設有證據。

回覆包含 `protocol`、`id`、`method`、`status`、`action_allowed: false` 和 `result`。`ok` 以狀態碼 0 結束，只表示唯讀呼叫完成。`unknown` 或 `unsupported` 以狀態碼 3 結束。`observation_incomplete` 或 `error` 以狀態碼 1 結束，須檢查原因及可能保留的證據。結果不能授權操作。

## 經由 ReachAgent App 的路徑

`capture`、`capture_all_windows`、`accessibility`、`accessibility_subtree` 四個方法，在請求**沒有** `capture_helper` 也**沒有** `ax_helper` 時，改由 ReachAgent App（見 [ReachAgent 服務協定](AGENT-SERVICE.md)）處理，權限因此算在 App 的簽章身分上。只要請求帶任一個 helper 欄位，就走上面原本的 helper 路徑，行為不變。

```sh
printf '%s\n' '{"id":"ax-1","method":"accessibility","pid":12345}' | target/debug/cu-agent
printf '%s\n' '{"id":"cap-1","method":"capture","pid":12345,"window_id":678,"image":"/absolute/new.png"}' | target/debug/cu-agent
```

- 欄位：`capture` 與 `capture_all_windows` 只有 `pid`、`window_id`、`image`；`accessibility` 只有 `pid` 與選用的 `windows_only`；`accessibility_subtree` 只有 `pid`、`source`、`path`。`describe` 的 `agent_fields` 與 `agent_optional_fields` 列出這些欄位。多出、缺少或型別錯誤的欄位一律拒絕，且在連線之前就拒絕。
- Socket 路徑：環境變數 `REACH_AGENT_SOCKET`（絕對路徑）；未設定時為 `$HOME/.reach/agent.sock`。路徑最長 100 位元組；路徑無效（不是絕對路徑、太長、沒有 `HOME`）是設定錯誤，回報 `error`、狀態碼 1，不是 `agent_unavailable`。`cu-agent` 不建立目錄、不啟動 App；預設目錄 `~/.reach` 由使用者自己建立（`mkdir -m 700 ~/.reach`），它必須屬於使用者本人且其他人不可寫，否則服務啟動時會拒絕。
- 回覆：App 回傳的報告原樣放在 `result`。`capture` 與 `capture_all_windows` 另外加上 `image_sha256`（`cu-agent` 讀取該 PNG 計算）。`cu-agent` 會用 `check_*_result` 核對報告與請求一致，不一致回報 `error`。
- 狀態：`accessibility` 成功為 `ok`、狀態碼 0；`capture`、`capture_all_windows`、`accessibility_subtree` 成功為 `unknown`、狀態碼 3（視窗綁定與內容仍未驗證）；App 回報錯誤（例如 `permission_denied`）為 `error`、狀態碼 1，`result.agent_error` 帶 `code`、`message` 與（若有）`permission`、`state`。
- App 沒有執行：回報 `agent_unavailable`、狀態碼 3，`result` 有 `reason`、`socket`、`start` 與 `fallback: "none"`。`start` 是給人看的啟動指令，形如 `open -a ReachAgent --args --serve --socket <路徑>`；Reach 不會自己啟動 App，也**不會**退回 helper 路徑或使用 terminal 的權限。
- `describe` 的 `agent` 欄位回報 App 狀態：`running`（含 `app_version`、`protocol_version`、`test_mode` 與三種權限）、`unavailable` 或 `error`。它只呼叫 App 的 `describe` 與 `permissions`，不要求任何權限。
- 真機行為尚未驗證：需要使用者建立憑證並授予權限（issue #5、#6）。目前只有假 socket 的離線測試。

呼叫者提供的 helper 路徑會被執行；只可使用受信任的執行檔。此 CLI 不驗證執行檔來源或雜湊，也不能防止它們被替換。它不證明目標沒有遮擋、座標已校準，或輸入不會影響游標與焦點。安全背景輸入未驗證前，不得將回覆轉成點擊或按鍵。

AX 身分與快照 helper 的上限為 15 秒；擷取 helper 與遊戲 observer 的上限為 30 秒。輸出各限制 4 MiB。逾時時嘗試終止 helper 的程序群組，並回報錯誤。已離開群組的子程序可能繼續執行；若主程序先結束但後代仍持有輸出管道，逾時不保證清理後代。輸出管道採非阻塞讀取；超過期限會停止讀取。這不保證後代程序已結束。雜湊等其他子程序仍沒有此上限。呼叫端須檢查殘留程序和輸出檔，不得將逾時視為安全完成。

擷取圖片的雜湊直接使用 macOS CommonCrypto，不啟動外部雜湊程序。它不防止檔案在檢查後被更換，亦不代表畫面或座標已驗證。

### 持續監看焦點通知

`target/debug/mac-focus-notifications --watch` 是獨立 helper，不是 `cu-agent` 的單次 JSON 呼叫。以管道啟動並保持 stdin 開啟。helper 成功註冊三項通知、核對前景後，先輸出一行 `status=ready`。Agent 收到 ready 後才可開始受控觀察。期間若收到 `focus_changed` 行，helper 會自行結束並回報非零狀態。Agent 須停止相關操作並保留證據。完成後向 stdin 傳送 `stop\n`，再讀取最後一行結果及退出狀態。此模式只接受這個固定命令，最長 30 秒。遇到焦點通知、結束時前景不同、EOF、錯誤命令或逾時會停止並以非零狀態結束。

監看只針對最初的前景 App，不包含其他 App 的通知。ready 和結束結果包含 PID 與程序啟動時間；Rust 會核對兩者，缺少或不一致時停止。註冊失敗不會輸出 ready。通知可能遲到或遺漏；前後前景相同或零通知都不能證明焦點不中斷。這個功能不送輸入，也不允許自動操作。

`python3 test-focus-notifications-live.py` 是明確啟動的受控測試。它啟動同一 helper 的無視窗子程序，只對該子程序訂閱並發送一個 AX 測試通知。測試核對事件行、自動停止和子程序清理。它不接受外部目標 PID，也不送出滑鼠或鍵盤事件。成功只證明這個受控通知的路徑，不能證明實際 App 的通知完整或背景輸入安全。

### Rust 監看協調程式

`cu-focus-watch --helper /absolute/mac-focus-notifications --evidence /absolute/new.jsonl` 啟動可信的通知 helper。它以 JSON Lines 輸出 `ready`、焦點變更及結束結果，並同步保存到新的證據檔。收到 `ready` 後，Agent 才能開始受控觀察。輸入 `stop\n` 可結束；收到焦點變更時應立即停止相關工作。EOF、異常資料、helper 失敗或逾時會停止並記錄錯誤。程式只觀察，絕不發送輸入。

此程序只管理自己的 helper。helper 路徑由呼叫端信任，程式不驗證其來源。監看只涵蓋最初的前景 App，通知可能遺漏；`ready` 和正常停止都不授權輸入。若 helper 或後代異常，可能殘留程序；呼叫端需檢查。證據檔不得已存在。

`cu-focus-watch --self-test-notification --helper /absolute/mac-focus-notifications --evidence /absolute/new.jsonl` 僅供受控驗證：helper 啟動自己的無視窗子程序並發送一個 AX 測試通知。Rust 轉發通知並保存證據；預期以狀態碼 3 停止。此模式不接受目標 PID，不發送滑鼠或鍵盤事件，也不驗證其他 App 的輸入安全。

## Computer Use 唯讀對照

本節的「Computer Use」指另一個外部產品，內容是只讀的行為對照。Reach 不依賴它。

2026-11-09 對系統設定執行 `all_windows`，回報九個候選及一個因資料無效而排除的視窗。這只驗證一次列舉，不證明視窗身分或非螢幕擷取可靠。觀察紀錄（原始證據：來源專案 `benchmark/cross-app-20261109/README.md`，未隨本庫攜帶）。

同日對系統設定測試 `observe_all_windows`，四次身分取樣一致；取得九個候選並排除一個無效候選。沒有擷取畫面或送出輸入。這不是視窗持續綁定或輸入安全的證明。

同日實測 `record_all_windows`，產生本機證據檔；九個候選中另有一個無效候選被排除，因此正確回報 `observation_incomplete`。證據內容留在本機，沒有上傳。

同日實測 `capture_all_windows`：對當時螢幕上的遊戲視窗取得 2204 × 1484 圖片，畫面顯示連刷第六場戰敗。最初對系統設定的非螢幕 `About` 候選擷取時，因清單有一個無效項目而停止；改為單獨查詢目標後，取得 723 × 2850 圖片。Rust OCR 在該圖找到 `General`、`Storage`、`Displays`，和先前 Computer Use 無障礙樹的文字相符。這是一次非螢幕圖片內容檢查，未驗證時效或其他 App，也不證明輸入安全；系統設定圖片留在本機，未上傳。

同日實機負面測試，分別將遊戲 PID 與另一程序的視窗編號配對，以及指定不存在的視窗編號。兩者均回報 `observation_incomplete`、`target window missing or ambiguous` 和 `action_allowed=false`；各自保存錯誤 sidecar，未產生 PNG。這只覆蓋當次的錯誤身分，不證明可排除查詢間 PID 重用或其他競態。

同日兩個擷取同時啟動時都在 30 秒逾時，並保存失敗紀錄；沒有觀察到 helper 殘留。之後分別單獨執行，遊戲回報兩次 `onscreen=true`，系統設定的非螢幕候選回報兩次 `false`，兩者均擷取完成。原因未確定，這不證明並行一定失敗或單獨擷取一定成功；呼叫端應檢查逾時與殘留，不要假設狀態。

後續在 macOS 擷取 helper 的啟動處加入同帳號合作鎖。若另一個使用相同鎖的新版框架擷取仍在進行，新請求立即回報 `capture busy or lock failed`，既有觀察流程會保存失敗 sidecar，不自動重試。鎖檔位於 `/private/tmp/mac-computer-use-capture-<effective-uid>.lock`；鎖由系統在持有程序關閉檔案時釋放。不要刪除或替換鎖檔。它不是同帳號惡意程式的安全邊界，不協調舊版程式或其他 App 的擷取；helper 若逾時且有脫離程序群組的後代，鎖釋放後仍不能保證後代已結束。

離線測試涵蓋重入、釋放及符號連結拒絕。跨程序實測中，測試程序持鎖時，遊戲擷取回報忙碌並保存錯誤，沒有產生圖片；結束測試程序後，遊戲擷取完成。這不能證明原先逾時的根因，也不保證未來不會逾時或授權輸入。

同日另以假擷取 helper 測試兩個實際 CLI：通用 CLI 進入 helper 後，遊戲 CLI 在前者仍執行時回報 `observation_incomplete` 和擷取忙碌，保留擷取證據且未建立圖片。假 helper 不擷取畫面；結束後其程序已回收。這次只驗證兩個當時建置的 CLI 共用鎖，不涵蓋其他程式或所有故障。

2026-11-09，依 bundle ID 或名稱選取某個遊戲 App 時，Computer Use 未找到執行中的 App；使用當時 `find_bundle_windows` 回報的 `app_bundle_path` 則可取得視窗及截圖。AX 樹只有遊戲視窗外層。這只是一次唯讀對照；每次應重新查找，不能固定暫存路徑。Computer Use 的 `getApp` 在目標未執行時可能啟動 App，因此在使用者操作或身分不確定時不要自動呼叫。截圖和視窗資料仍未校準，不能據此點擊。

同日系統設定的 `AXWindows` 回報空清單，但主要與焦點視窗指向同一元素；該元素的程序、標題及未校準幾何與當時的 `About` 候選相符。新版唯讀備援列出一個來源為 main 和 focused 的 AXWindow，共讀到 77 個節點，且回報 `truncated=true`。這是一次非原子取樣，不證明視窗綁定或完整樹。遊戲當時原本的 AXWindows 清單有一個視窗，沒有啟動備援。

同日對系統設定其中一個先前截斷的備援分支使用 `accessibility_subtree`，讀到 3 個節點且該次未截斷；不存在的分支回報錯誤。沒有執行無障礙動作。這不保證路徑在後續查詢時仍指向同一元素。

同日新增 `windows` 來源後，對遊戲的第一個 AX 視窗做唯讀測試，取得 5 個節點且回報未截斷；不存在的第二個視窗索引回報錯誤。這不代表遊戲內按鈕均可透過無障礙讀取，也不能授權操作。

同日比較當前遊戲的 Computer Use 與 Rust 截圖：前者 JPEG 1102 × 742，後者 PNG 2204 × 1484。畫面和時點並非原子一致，且 Computer Use 圖片未保存；尺寸相差兩倍不能當成點擊座標校準。觀察紀錄（原始證據：來源專案 `benchmark/cua-rust-size-20261109/README.md`，未隨本庫攜帶）。

2026-11-09：Finder 的 AppKit `launchDate` 在本機回傳空值，造成舊版 helper 拒絕查找。新版 AX helper 對查找、程序身分及 AX 樹一致使用核心程序啟動時間；讀取失敗仍停止。啟動時間使用浮點秒，不能單獨證明程序身分，也不可將不同版本 helper 的值混用。須重新執行 `sh build-ax.sh`。唯讀測試中 Finder 與系統設定的查找、身分及 AX 查詢所得時間一致；Finder 回報一個 AX 視窗，但兩個 App 當次均沒有螢幕視窗候選。這是有限取樣，不能證明畫面或輸入安全。

程序身分現在包含 `process_start_source=kernel_sysctl_kern_proc`。新版 Rust macOS 後端拒絕缺少來源或來源不同的 AX helper；視窗查找與擷取、AX 比對也要求此來源一致。舊紀錄不會自動補上來源。更新後須重新建置 AX helper、通用 CLI 和使用此框架的遊戲 CLI。此欄位是 helper 的資料來源標記，不證明程序持續不變，也不授權輸入。

### 依 App 查詢全部視窗候選

`find_bundle_all_windows` 接受 `bundle_id` 和可信的絕對 `ax_helper` 路徑。它在每個符合條件的程序前後取樣，再列舉全部視窗候選，包括非螢幕視窗。`inventories` 分別記錄每個程序的候選數與 `omitted_invalid_count`。`all_inventory_calls_completed` 只表示各次查詢有回覆，不代表清單完整；應同時檢查排除數與 `same_complete_samples`。中途失敗時保留已取得的候選，`all_inventory_calls_completed=false`。結果始終是 `unknown`、狀態碼 3、`action_allowed=false`。視窗是否可見、內容新鮮、身分持續綁定及輸入安全都未證明。

2026-11-09 唯讀實測，Finder 回報 14 個候選，遊戲回報 12 個；兩次查詢的排除數均為 0。這只是各一次的清單，不代表每個候選都能擷取或操作。

`record_bundle_all_windows` 接受 `bundle_id`、可信的絕對 `ax_helper` 路徑和新的絕對 `output` 路徑。它先建立權限 0600 的新檔，再執行 `find_bundle_all_windows` 並同步保存結果或錯誤。既有檔案不覆寫。`query_complete=true` 只表示程序取樣一致、各次列舉有回覆且沒有回報被排除項目；零個程序也可能符合這些條件。它不保證清單完整、找到目標、畫面新鮮或操作安全。查詢不完整時回報 `observation_incomplete` 和狀態碼 1；其他情況仍是 `unknown` 和狀態碼 3。程序中斷或寫入失敗可能留下空檔或部分檔案。結果可能含視窗標題、程序路徑與座標，應保護證據檔。

2026-11-09 唯讀測試 Finder：保存 14 個候選，排除數為 0，`query_complete=true`；再次指定同一證據檔時停止，沒有覆寫。這不是視窗綁定或背景輸入安全的證明。

### 影格串流狀態

先執行 `sh build-frame-status.sh`，並重新建置 `cu-agent`。`record_stream_status` 接受 `pid`、`window_id`、可信的絕對 `stream_helper` 和 `ax_helper` 路徑，以及新的絕對 `output` 路徑。它先建立證據檔，再取得與截圖共用的合作鎖。Rust 在 helper 前後核對程序身分、目標視窗、層級與幾何。helper 以非啟用模式取得最多一筆串流影格，並核對前後的 ScreenCaptureKit 視窗和螢幕狀態；它不保存圖片或傳送輸入。helper 預設最多等待三秒取樣，Rust 設有 20 秒外部期限。

`sample_complete=true` 表示上述有限核對通過，且第一筆影格狀態為 complete，有影像緩衝區及必要時間欄位。它回報 `unknown`、狀態碼 3。暫停、空白、等待逾時、身分改變或資料錯誤時為 `observation_incomplete`、狀態碼 1，已取得的資料與錯誤會保存。證據檔不能覆寫；中斷或寫入失敗可能留下部分檔案。helper 路徑由呼叫端信任；逾時後脫離程序群組的後代仍可能殘留。鎖只協調使用同一鎖的新版程式。

這不是截圖，也沒有和其他截圖綁定。complete 只表示系統產生影格；不證明遊戲內容更新、視窗持續不變或輸入隔離。`action_allowed` 始終為 false。

2026-11-09 對遊戲做一次唯讀測試，串流狀態為 complete 並保存證據。另對一個非螢幕 Finder 候選測試時，單一視窗核對回報缺少或不唯一，因此在啟動串流前停止並保存失敗；此結果不代表 Finder 當時的串流狀態。

2026-11-09 修正單一視窗查詢：不再單獨使用 `optionIncludingWindow`，改用 `CGWindowListCreateDescriptionFromArray` 查詢指定 ID。修正後一次唯讀測試中，先前失敗的非螢幕 Finder 候選通過查詢並回報串流 suspended；遊戲候選回報 complete。錯誤擁有者及不存在的 ID 均在串流啟動前停止。測試紀錄（原始證據：來源專案 `benchmark/stream-exact-api-20261109/README.md`，未隨本庫攜帶）。視窗查詢結果仍可能過期，不代表安全輸入。

### 同一串流影格的圖片與狀態

執行 `sh build-stream-image.sh` 並建置 `cu-agent`。`capture_stream_image` 接受 `pid`、`window_id`、可信的絕對路徑 `stream_helper`、`ax_helper`，以及新的絕對 PNG 路徑 `image` 和新證據路徑 `output`。程式先預留證據檔，並取得與其他擷取共用的合作鎖。它在 helper 前後核對程序身分、單一視窗、層級及幾何。helper 從同一筆串流 sample 讀取狀態與圖片，只對 complete 影格輸出 PNG。Rust 核對回報的目標、幾何與圖片尺寸，並保存 PNG 的 SHA-256。成功仍回報 `unknown`、狀態碼 3 和 `action_allowed=false`。

遇到暫停、缺少資料、目標變更或 helper 失敗，回報 `observation_incomplete`、狀態碼 1，並保留取得的狀態與錯誤。若圖片已建立但後續驗證失敗，圖片會保留，所有權不明；不要自動重試或刪除。檔案不得已存在。中斷或寫入失敗可能留下部分檔案。helper 路徑由呼叫端信任；逾時後脫離程序群組的後代仍可能殘留，鎖也只協調使用同一鎖的新版程式。

這項擷取只證明程式從同一筆 sample 取得資料，沒有校準像素與視窗座標。與單張截圖相比，內容邊界曾有差異。完整影格不代表 App 內容已更新；非原子程序核對也不證明持續綁定。不可用結果授權輸入。

2026-11-09 唯讀實測：遊戲回報完整影格，已保存圖片、狀態及符合檔案的雜湊；非螢幕 Finder 候選回報 suspended，沒有建立圖片。這是兩個候選各一次取樣。測試證據（原始證據：來源專案 `benchmark/stream-image-integrated-20261109/`，未隨本庫攜帶）。

`capture_stream_image` 的 helper 現在會在同一筆影格的 `sample.first` 中附上可讀取的 `content_rect_points`、`bounding_rect_points`、`content_scale` 和 `scale_factor`；缺少或無效時為 null。它也記錄 filter 的內容矩形與像素比例。這些欄位只是可選的原始幾何診斷，Rust 不以它們授權或換算輸入，也不因它們缺少就推斷可用座標。舊版 helper 不含這些欄位，需重新執行 `sh build-stream-image.sh`。建置腳本會先執行不擷取畫面的幾何自測。

2026-11-09 對遊戲實測，影格附件的 content scale 約為 0.959；非螢幕 Finder 候選暫停時未提供幾何。這是各一次取樣，不能當成固定轉換。觀察紀錄（原始證據：來源專案 `benchmark/stream-geometry-20261109/README.md`，未隨本庫攜帶）。

同影格擷取成功後，Rust 在串流證據及外層擷取證據加入 `geometry_diagnostic`。若必要的 content scale、scale factor、filter scale 和矩形可讀且有效，狀態為 `reported_unverified`，並以 `content_scale_exactly_one` 表示系統回報值是否恰為 1。若欄位缺少或無效，狀態為 `unavailable`。這是對 helper 回報的整理，不是固定轉換或座標校準；即使為 1 也不代表兩種截圖幾何相同。兩種狀態的 `action_allowed` 均為 false。

2026-11-09 以遊戲觀察流程實測，串流與外層證據攜帶相同診斷，content scale 約 0.959 且 `content_scale_exactly_one=false`。模型與 OCR 仍須 Agent 覆核。這是一次取樣，不能用於輸入。

同影格 helper 使用 `SCStream.synchronizationClock` 與 Core Media 主機時鐘，計算該 sample 到 callback 時的年齡，保存在 `sample.first.sample_age_at_receipt_ms`。若時鐘不可用、轉換無效或出現負值，欄位為 null。Rust 在成功擷取後輸出 `timing_diagnostic`：資料有效時為 `reported_unverified`，否則為 `unavailable`。此數值只描述 callback 當下，不是 Agent 讀取時的年齡，也不證明 App 內容已更新；沒有依此啟用輸入或設定未校準的逾時門檻。舊版 helper 不含此欄位，需重新建置。

2026-11-09 唯讀測試中，遊戲與系統設定各一次的 sample age 約 6 毫秒。遊戲流程的串流與外層擷取證據帶有相同時間診斷；系統設定圖片只在本機暫存後移除。這些不是準確率或隔離測試。

新版同影格 helper 另記錄從串流時鐘轉換的 `sample_host_ticks` 和 callback 時的 `receipt_host_ticks`。Rust 在同一執行期間使用 macOS 主機時鐘與 timebase 計算 `sample_age_at_record_ms`。若時鐘不可用、先後順序不符、timebase 無效，或換算後 callback 年齡和 helper 回報相差超過 1 毫秒，`record_age_status=unavailable`，不猜測結果。舊版 helper 沒有 ticks，也會回報 unavailable。此檢查只整理資料；不以此授權輸入，也沒有套用未校準的時間上限。

2026-11-09 遊戲唯讀測試中，影格 callback 年齡約 8 毫秒，Rust 完成程序與圖片核對時約 298 毫秒。兩者是同一影格的兩個不同時間點；後續 OCR、模型與 Agent 閱讀還會再增加時間。原始 ticks 不能在系統重啟後與另一執行期間的時鐘直接比較。即使時間很短，也不代表遊戲內容更新或視窗、輸入隔離安全。測試紀錄（原始證據：來源專案 `benchmark/game-record-age-20261109/README.md`，未隨本庫攜帶）。
