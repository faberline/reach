# ReachAgent 服務協定

ReachAgent 是 reach 之後要用固定身分的 App 執行的小服務。需要系統權限的動作（螢幕錄製、輔助使用）交給它，這樣權限算在 App 上，而不是算在 terminal 或 agent 的 shell 上（背景見 [BUILD.md](BUILD.md) 的「權限與簽章」）。

目前有協定、服務骨架（faberline/reach-old#33）、視窗擷取（faberline/reach-old#35）與無障礙快照（faberline/reach-old#39）：App 包裝與簽章是 faberline/reach-old#34；`cu-agent` 的路由（不帶 helper 欄位的 `capture`、`capture_all_windows`、`accessibility`、`accessibility_subtree` 改走本服務）見 [AGENT-CLI.md](AGENT-CLI.md)。服務不送任何輸入事件，啟動時也不會要求任何權限；只有對應權限已是 `granted` 時，`capture` 與 `capture_all_windows` 才會擷取指定的單一視窗，`accessibility` 與 `accessibility_subtree` 才會讀取指定行程的無障礙樹（唯讀）。

## 啟動與自我測試

```sh
sh build-agent.sh
target/debug/ReachAgent --serve --socket <絕對路徑> [--test-mode]
target/debug/ReachAgent --self-test
```

- `sh build-agent.sh` 建置 `target/debug/ReachAgent` 並執行 `--self-test`。
- `--serve --socket <路徑>`：監聽 Unix domain socket。路徑必須是絕對路徑、長度不超過 100 位元組，其父目錄必須屬於目前使用者且其他人不可寫。路徑已存在時，只有它是自己的 socket 才會被取代；其他檔案一律不動，並以 `path_not_socket` 結束。socket 檔的權限是 0600。
- `--test-mode`：不呼叫任何系統權限 API，三種權限固定回 `unknown`，`request_permissions` 什麼都不要求；因為 `screen_recording` 與 `accessibility` 都是 `unknown`，`capture`、`capture_all_windows`、`accessibility` 與 `accessibility_subtree` 一律回 `permission_denied`。CI 使用這個模式。
- `--self-test`：在暫時目錄的 socket 上以測試模式啟動服務，涵蓋所有方法與錯誤情境、超長行、目錄與檔案權限、不覆蓋既有檔案，再清除。擷取只用假後端（合成的 2×2 PNG），無障礙只用假後端（合成的報告），並在測試中把 `screen_recording` 與 `accessibility` 暫時設為 `granted` 或 `denied`；另對移植的純函式做自我檢查。不需要任何權限，不擷取螢幕，也不讀其他行程的無障礙樹。

服務一次處理一條連線，只接受與服務同一個使用者的連線（`getpeereid`），其他使用者收到 `peer_uid_mismatch` 後連線關閉。每條連線的讀取與寫入逾時各為 30 秒。

## 傳輸與格式

每個請求一行 JSON（以 `\n` 結尾），每個回應一行 JSON。一條連線可以連續送多行，依序回應。單行上限 65536 位元組；超過時回一則 `invalid_json` 並關閉該連線。

請求：

```json
{"version":1,"id":"<字串>","method":"describe"}
```

`params` 可省略（視為 `{}`）。`id` 必須是字串，原樣帶回。

成功：`{"version":1,"id":"…","ok":true,"result":{…}}`。

錯誤：`{"version":1,"id":"…或 null","ok":false,"error":{"code":"…","message":"…"}}`。`permission_denied` 另有 `error.permission` 與 `error.state`。

## 方法

| 方法 | params | result |
|---|---|---|
| `describe` | 無 | `app`、`app_version`、`protocol_version`、`methods`、`test_mode` |
| `permissions` | 無 | `accessibility`、`screen_recording`、`post_event`，值為 `granted`、`denied` 或 `unknown`（只預檢，不彈窗） |
| `request_permissions` | `{"permissions":["accessibility","screen_recording"]}` | `requested`（實際要求的名稱）與 `permissions`（三種狀態） |
| `capture` | `{"pid":<整數>,"window_id":<整數>,"image":"<絕對 .png 路徑>"}` | 擷取報告（見下），只在螢幕上的視窗中比對 |
| `capture_all_windows` | 同 `capture` | 同 `capture`，但在所有視窗（含不在螢幕上的）中比對；`window_scope` 為 `all_window_candidates` |
| `accessibility` | `{"pid":<整數>,"windows_only":<布林，可省略>}` | 行程的 AX 視窗快照（見下） |
| `accessibility_subtree` | `{"pid":<整數>,"source":"main"、"focused" 或 "windows","path":[<整數>,…]}` | 單一子樹的快照（見下） |
| `input_source` | 無 | 目前的鍵盤輸入來源（唯讀，見下方「唯讀觀測」） |
| `secure_input` | 無 | 安全輸入（Secure Event Input）是否開啟（唯讀） |
| `pointer_location` | 無 | 指標位置（唯讀，不移動指標） |
| `bundle_windows` | `{"bundle_id":"<bundle id>"}` | 該 bundle id 在螢幕上的視窗，含 layer 與 bounds（唯讀，不讀標題） |

只有明確呼叫 `request_permissions` 才會向系統要求權限。預設 `post_event` 不能要求，含它的請求回 `not_requestable`；只有以輸入模式啟動（見下方「輸入模式」）時，`post_event` 才能被明確要求。

## 輸入模式（計畫 I-2，預設停用）

**預設不送任何輸入事件。** 沒有 `--enable-input` 時，服務的行為與之前完全相同：`describe` 沒有 `action` 方法，`input` 區塊只有 `{"state":"disabled","backend":…}`，`action` 回 `unknown_method`，`post_event` 不能要求。

```sh
target/debug/ReachAgent --serve --socket <絕對路徑> [--test-mode] \
  --enable-input --allow-bundle <bundle id> [--allow-bundle <bundle id>] \
  [--allow-keycode <18|23|36|38|44|49>...] --input-ttl-seconds <1-600> --max-input-events <1-20>
```

- 啟用一定要同時給 `--allow-bundle`、`--input-ttl-seconds` 與 `--max-input-events`。沒有 `--enable-input` 卻給輸入選項、未知選項、重複選項，都是用法錯誤。
- `--allow-bundle` 只接受 `org.local.rust-computer-use.target-event-fixture` 與 `com.apple.Safari`，沒有萬用字元；Chrome 未核准。
- 到期最多 600 秒，事件數最多 20。鍵碼只接受計畫 1.3 與 Q10 列出的六個：18、23、36、38、44、49（注音鍵 18、23、38、44，Return 36，空白 49）；修飾鍵（54 至 63，含 Fn、Caps Lock）、功能鍵與系統鍵一律拒絕，啟用時與事件解析時各檢查一次。到期或事件數用完後，閘門拒絕所有請求，要以新的啟動重新啟用。MCP 不能啟用輸入。
- 只有三種事件：`pointer_move`、左鍵 `click`、無修飾鍵的 `key_tap`。沒有拖曳、滾輪、修飾鍵、文字輸入、啟用 App。
- `action` 的請求與閘門 G1 至 G13 與離線模型相同（[FOREGROUND-INPUT-MODEL.md](FOREGROUND-INPUT-MODEL.md)）。結果有 9 個欄位：`status`、`action_allowed`（永遠 `false`）、`gate`、`reason`、`attempted`、`release_attempted`、`release_confirmed`、`attempt_file`、`result_file`。成功投遞也只是 `posted_unverified`，不是驗證。`delivery_unknown` 之後閘門停機（G13）。
- `action` 先檢查 `post_event`：不是 `granted` 就回 `permission_denied`，不進閘門。
- `describe` 的 `input` 區塊：`state`（`armed`）、`backend`（`fake` 或 `system`）、`allowed_bundle_ids`、`allowed_keycodes`、`expires_unix_seconds`、`events_remaining`、`halted`、`verified`（永遠 `false`）。

### 兩種後端

- **假後端**（`--test-mode --enable-input`）：不呼叫任何系統 API，只記錄「會送出的事件」，收據寫進記憶體。`--self-test` 用它涵蓋啟用、到期、允許清單、拒絕、重放防護、單一事件規則、停機與收據。
- **系統後端**（`ReachAgentInputSystem.swift`）：唯一可以呼叫事件投遞 API 與 `CGRequestPostEventAccess` 的檔案。它被編譯進 App。啟動時 `SystemInputLaunch.authorize` 在有 `--enable-input`、沒有 `--test-mode`、行程環境變數 `CI` 為空時才回傳值；它自己讀 `ProcessInfo.processInfo.environment`，呼叫端不能傳入環境。`--enable-input` 不帶 `--test-mode` 而在 CI 下啟動時，服務以 `input_unavailable` 結束。
- 這些檢查是什麼、不是什麼：
  - `--test-mode`、`CI` 檢查與靜態測試（`tests/foreground_input_static.rs`：投遞 API 只出現在這個檔案、沒有被禁止的事件種類、`SystemInputLaunch.authorize(` 與 `makeInputController(` 的呼叫點逐行列舉）都是**防誤用**的護欄，不是安全邊界。任何能在這台機器以 `--enable-input` 且不帶 `--test-mode` 啟動 ReachAgent、又沒有設定 `CI` 的人，都會拿到系統後端；修改原始碼的人也能改掉靜態測試。
  - 目前真正讓系統後端送不出事件的，是即時事實的失敗即關閉：系統後端只唯讀取樣目標 App 的 bundle id 與是否為最前面的 App（G8，`NSWorkspace.frontmostApplication`）；焦點、HID 閒置、安全輸入、輸入法與遮擋的即時事實都沒有接上，一律視為不安全，所以每個請求都會在 G7、G8 或 G9 被拒絕。
  - I-3 只加入了唯讀觀測方法（見下方「唯讀觀測」），**沒有**把它們接進閘門的即時事實：G7 至 G9 仍然失敗即關閉。接上要等 I-4 在實機上以 fixture 驗證這些讀值之後；接上之後這一點就不再成立，屆時條件都符合的請求會真的投遞事件，能擋住它的只剩啟用參數、允許清單、事件數與到期這些閘門，以及使用者給的 TCC 授權。
- `--self-test` 只驗證 `--test-mode` 或沒有 `--enable-input` 時 `authorize` 一定回 nil，以及參數解析把 `--enable-input` 不帶 `--test-mode` 解析成系統路徑；它從不以 `testMode: false` 建立輸入控制器。`CI` 檢查讀真實環境，所以只有靜態測試涵蓋。系統後端從未實際執行過；真機驗證需要簽章與 TCC 授權，屬於 I-4 之後。

### 擷取

- params 只能有 `pid`（1 到 2147483647 的 JSON 整數）、`window_id`（1 到 4294967295 的 JSON 整數）與 `image`（絕對路徑、以 `.png` 結尾、最長 1024 位元組、不含 NUL）；缺少、多出、型別錯誤或超出範圍都回 `invalid_params`，且先於權限檢查。
- 服務先檢查 `screen_recording`：不是 `granted` 就回 `permission_denied`（`error.permission` 為 `screen_recording`），這時不呼叫後端，也不建立任何檔案。服務自己不要求權限，也不彈窗。
- 只擷取 `pid` 與 `window_id` 都符合且唯一的視窗（`SCContentFilter(desktopIndependentWindow:)`），不擷取整個螢幕。擷取前後會比對視窗幾何與是否在螢幕上，不符就失敗。
- PNG 以排他方式建立（權限 0600，不覆寫既有檔案，不跟隨符號連結）。寫入失敗時服務會移除半成品；其他失敗之後該路徑是否留有檔案，呼叫端要自己檢查，服務不重試。整個擷取最多 30 秒，逾時回 `capture_failed`，並取消擷取工作（寫檔前會檢查取消，但已開始寫檔的那一刻無法中止）。
- result 欄位：`window_id`、`owner_pid`、`window_frame_points`、`filter_content_rect_points`（`x`、`y`、`width`、`height`）、`point_pixel_scale`、`image_width`、`image_height`、`capture_start_uptime_ns`、`capture_end_uptime_ns`、`capture_end_unix_seconds`、`image_path`、`capture_type`（`desktop_independent_window`）、`window_scope`、`onscreen_before`、`onscreen_after`、`occlusion_checked`（固定 `false`，沒有檢查遮擋）。欄位與舊的單張擷取 helper 相同，不含 `image_sha256`：`cu-agent` 轉發時會讀取該 PNG 計算並加入雜湊，證據檔仍由呼叫端保存。Rust 端的 `check_capture_result` 核對結果與請求一致。
- 真機擷取（`SystemCaptureBackend` 實際運作）尚未驗證：需要使用者建立憑證並授予螢幕錄製（issue #5、#6）。目前只有假後端的離線測試。

### 無障礙快照

- `accessibility` 的 params 只能有 `pid`（1 到 2147483647 的 JSON 整數）與選用的 `windows_only`（JSON 布林，預設 `false`）。`accessibility_subtree` 的 params 只能有 `pid`、`source`（`main`、`focused` 或 `windows`）與 `path`（最多 8 個 0 到 159 的整數，每個是往下一層的子節點索引；`source` 為 `windows` 時第一個索引是視窗索引，必須在 0 到 31，因此 `path` 不可為空）。缺少、多出、型別錯誤或超出範圍都回 `invalid_params`，且先於權限檢查。
- 服務先檢查 `accessibility`：不是 `granted` 就回 `permission_denied`（`error.permission` 為 `accessibility`），這時不呼叫後端。服務自己不要求權限，也不彈窗；兩種權限互相獨立。
- 後端只讀：使用 `AXUIElementCopyAttributeValue` 等讀取函式，不執行 AX 動作、不設定屬性、不送輸入事件。節點數上限 160、深度上限 6、單一字串 240 字元、動作名稱 32 個、視窗 32 個、整體 5 秒預算；超過時 `truncated` 為 `true`。
- `accessibility` 的 result：`schema_version`（1）、`status`（`ok`）、`pid`、`process_start_unix_seconds`、`process_start_source`、`bundle_id`、`executable`、`scope`（`process_ax_windows`）、`tree_scope`（`windows_only` 或 `full_bounded_tree`）、`cg_window_binding`（固定 `unverified`）、`window_count_reported`、`node_count`、`truncated`、`action_allowed`（固定 `false`）、`windows`（最多 32 個）與 `fallback`（`null` 或物件）。AX 視窗與 `capture` 的 `window_id` 之間的對應尚未驗證，所以 `cg_window_binding` 固定為 `unverified`。
- `accessibility_subtree` 的 result：同樣有 `schema_version`、`status`、`pid`、行程啟動時間，另有 `source`、`path`（與請求相同）、`node`、`node_count`、`truncated`、`scope`（`read_only_subtree_unverified`）、`cg_window_binding` 與 `action_allowed`。
- 後端失敗（行程不存在或已改變、AX 視窗讀不到、子樹路徑無效等）回 `ax_failed`，`message` 為 `<狀態碼>: <原因>`，例如 `process_unavailable: …`。
- 每次呼叫都重新讀取無障礙樹，不保證兩次呼叫之間同一個 `path` 指向同一個元素。結果只是唯讀的觀察，不授權任何操作。
- Rust 端的 `check_accessibility_result` 與 `check_subtree_result` 核對結果與請求一致。
- 真機無障礙讀取（`SystemAccessibilityBackend` 實際運作）尚未驗證：需要使用者建立憑證並授予輔助使用（issue #5、#6）。目前只有假後端的離線測試，也尚未驗證主執行緒阻塞在 accept 時 AX 讀取的行為。

### 唯讀觀測（計畫 4.5，I-3，實機未驗證）

`input_source`、`secure_input`、`pointer_location` 與 `bundle_windows` 只讀狀態（`ReachAgentObservation.swift`）。它們不需要任何權限、不檢查也不要求權限、不會彈出系統提示；不切換、不啟用、不列舉選擇輸入來源，不移動指標，不送任何事件，不讀視窗標題。

- params：前三個方法不能有 params，或只能是空物件 `{}`；`bundle_windows` 只能有 `bundle_id`（1 到 255 位元組的 ASCII 字母、數字、`.` 與 `-`）。其他情形回 `invalid_params`，且先於任何讀取。
- 每個 result 都有 `backend`（`fake` 或 `system`）、`scope`（固定 `read_only_unverified`）、`action_allowed`（固定 `false`）與 `used_by_input_gate`（固定 `false`）。
- `input_source`：`input_source_id`、`input_mode_id`、`localized_name`、`source_type`（後三個可為 `null`），以及固定 `false` 的 `selection_changed`。系統後端呼叫 `TISCopyCurrentKeyboardInputSource` 並讀其屬性，只在主執行緒讀。
- `secure_input`：`secure_event_input_enabled`（`IsSecureEventInputEnabled`）。
- `pointer_location`：`x`、`y`（有限數）與 `coordinate_space`（`global_top_left_points`）。系統後端讀 `NSEvent.mouseLocation`，再以主螢幕高度翻成左上原點，與視窗 bounds 同一座標系。
- `bundle_windows`：`bundle_id`、`owner_pids`（最多 8 個）、`windows`（最多 64 個，每個有 `window_id`、`owner_pid`、`layer`、`bounds`、`alpha`、`z_index`）、`truncated`、`window_scope`（`onscreen_only`）、`window_titles`（`not_read`）、`skipped`（屬於該 bundle、但因視窗清單項目缺少 window number、layer 或 bounds 而被略過的視窗數；不再靜默略過）。每個視窗的 `alpha` 在視窗清單沒有提供時為 `null`，不會捏造成 1。系統後端讀 `CGWindowListCopyWindowInfo`（只取螢幕上的視窗、不含桌面元素），每次呼叫都以 `NSRunningApplication(processIdentifier:)` 比對擁有者的 bundle id；`z_index` 是在整份螢幕視窗清單中由前到後的位置。
- 後端讀不到時回 `observation_failed`。
- `--test-mode` 與 `--self-test` 只用假後端：固定的輸入來源、`secure_event_input_enabled: false`、指標 (100.5, 200.25)；fixture bundle id 有一個行程（pid 42）與 layer 0、layer 20 各一個視窗，`org.local.reach.fake-partial` 回一個 `alpha: null` 的視窗與 `skipped: 2`，`org.local.reach.fake-failure` 會失敗，其他 bundle id 沒有視窗。不呼叫任何系統 API。
- **閘門不使用這些讀值**：系統輸入後端的即時事實仍失敗即關閉（G7 至 G9 一律拒絕）。接進閘門要等 I-4 實機驗證。
- **實機未驗證**：系統後端的四個讀值都還沒在實機上與 fixture 對照驗證（座標翻轉在多螢幕下、輸入法 id 的值、安全輸入在其他 App 開啟時、視窗清單的 layer 與 z 順序）。結果只是觀察，不授權任何操作。
- **實機未驗證（讀值可能過時）**：服務的主執行緒停在 `accept()`、不跑 run loop，`input_source` 讀到的目前輸入來源在切換輸入法之後可能還是舊值。實機驗證時要先切換輸入法，再讀一次比對。
- Rust 端的 `check_observation_result` 核對結果（含 `bundle_windows` 與請求的 bundle id 一致、上限與固定欄位）。

## 錯誤碼

| code | 何時 |
|---|---|
| `invalid_json` | 不是單行 JSON 物件、有重複鍵、超過上限或不是 UTF-8 |
| `missing_field` | 缺 `id`、`version` 或 `method`，或型別不對 |
| `version_mismatch` | `version` 不是 1 |
| `unknown_method` | 不在上表的方法 |
| `invalid_params` | `request_permissions` 的 `permissions` 缺少、為空、型別錯誤、名稱未知或重複；`capture`、`capture_all_windows`、`accessibility`、`accessibility_subtree`、`bundle_windows` 的 params 缺少、多出、型別錯誤或超出範圍；`input_source`、`secure_input`、`pointer_location` 帶了非空的 params |
| `not_requestable` | 要求 `post_event` |
| `peer_uid_mismatch` | 對端不是同一個使用者 |
| `permission_denied` | `capture`、`capture_all_windows` 在 `screen_recording` 不是 `granted` 時回傳；`accessibility`、`accessibility_subtree` 在 `accessibility` 不是 `granted` 時回傳 |
| `capture_failed` | 擷取失敗：視窗找不到或不唯一、幾何或是否在螢幕上在擷取期間改變、尺寸不支援、輸出檔已存在或無法建立、逾時 30 秒、系統低於 macOS 14 |
| `ax_failed` | 無障礙讀取失敗：行程不存在或在讀取期間改變、AX 視窗讀不到或型別不對、子樹路徑無效；`message` 為 `<狀態碼>: <原因>` |
| `observation_failed` | 唯讀觀測讀不到：沒有目前的輸入來源、不在主執行緒、主螢幕沒有 bounds、視窗清單無法取得 |

缺少權限時不降級：方法回 `permission_denied`，並指出是哪一項權限與目前狀態。

Rust 端的同一份協定在 `src/agent_protocol.rs`（平台無關，有單元測試）；`ReachAgent.swift` 必須使用相同的常數與錯誤碼，由該模組的測試檢查。
