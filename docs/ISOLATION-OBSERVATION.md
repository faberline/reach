# 唯讀隔離取樣

`ObserveIsolation.swift` 連續取樣十次前景 App、焦點元素、游標位置及螢幕視窗順序。結果只列出是否與第一筆相同，不保存 App 身分、焦點名稱或游標座標。它不啟動 App、不建立視窗、不投遞事件，也不讀取按鍵或文字輸入。

```sh
# 在儲存庫根目錄執行
mkdir -p /private/tmp/sw-isolation-bin
swiftc -module-cache-path /private/tmp/sw-computer-use-swift-cache ObserveIsolation.swift -o /private/tmp/sw-isolation-bin/observe-isolation
CARGO_HOME=/private/tmp/sw-cargo-cache cargo build --locked --offline --bin cu-agent
printf '%s\n' '{"id":"baseline","method":"sample_isolation","isolation_helper":"/private/tmp/sw-isolation-bin/observe-isolation"}' | target/debug/cu-agent
```

取樣不可取得時回報 `unknown`。取樣成功時也回報 `unknown`、狀態碼 3 和 `action_allowed=false`。它只觀察約半秒，可能漏掉瞬間變化；即使結果都相同，也不能證明輸入安全。視窗清單及焦點資訊可能因權限或系統狀態不完整。呼叫端提供的 helper 是可信執行檔，框架不驗證其來源。

2026-11-08 的授權環境測試無法取得焦點元素，回報 `focusUnavailable`。失敗的確切原因尚未確認。測試沒有送出任何輸入；因此尚無完整基準結果，也不能據此推定游標、焦點或視窗順序不變。

2026-11-08 進一步唯讀診斷：診斷程序的 `AXIsProcessTrusted` 回傳 true，系統焦點查詢回傳 `kAXErrorCannotComplete`（-25204）；對當時前景 App 的焦點查詢則成功。這不證明其他執行檔具有相同權限，也不確定系統查詢失敗的原因。

工具現會在系統焦點查詢失敗時，嘗試讀取前景 App 的焦點元素，並將來源標為 `front_app_only`。它不能代替系統全域焦點。若十次取樣來源不同，標為 `mixed`；`focus_unchanged` 為 `null`，表示無法比較，不能推定焦點曾改變。這兩種情況皆回報 `sampled_partial`，`system_focus_sampled=false`，不得視為完整隔離證據。

同日實測十次取樣來源混合；取樣到的前景、游標和視窗順序沒有變化，但全域焦點未完整取得。測試沒有送出任何輸入。取樣空檔中仍可能有變化，不能證明背景輸入安全。

取樣也記錄單次最長時間和最長間隔。超過 250 毫秒時標為部分取樣；這是未校準的篩選條件，不代表較短間隔能排除瞬間變化。2026-11-08 對混合來源進行唯讀測試，`focus_unchanged` 為 `null`，沒有送出輸入。

2026-11-09 查閱 Apple 的 [`kAXFocusedApplicationAttribute`](https://developer.apple.com/documentation/applicationservices/kaxfocusedapplicationattribute) 說明後，另以系統層級焦點 App 作為入口測試。十次查詢均回傳 `kAXErrorCannotComplete`（-25204），因此沒有取得 App 的焦點元素，也沒有送出輸入。Apple 將此錯誤描述為無障礙訊息傳遞無法完成；這次結果不足以判定原因，不能將前景 App 替代為已驗證的系統焦點。[錯誤說明](https://developer.apple.com/documentation/applicationservices/1462085-axuielementcopyattributevalue)。

診斷程式為 `fixtures/SystemFocusDiagnostic.swift`。它只回報成功與錯誤次數，不輸出 App 身分或焦點內容。可在授權環境編譯並執行：

```sh
swiftc -module-cache-path /private/tmp/sw-computer-use-swift-cache fixtures/SystemFocusDiagnostic.swift -o /private/tmp/sw-system-focus-diagnostic
/private/tmp/sw-system-focus-diagnostic
```

同日另以 `fixtures/SystemFocusTimeoutDiagnostic.swift` 在同一程序中設定 0.1、0.5、1.5 秒逾時，各查詢三次系統焦點 App。程序回報 `AXIsProcessTrusted=true`，三種逾時設定均成功，但九次查詢全部立即回傳 -25204，最長約 0.000012 秒。這次延長逾時沒有解決問題；不能據此判定系統錯誤根因，也不能推定其他程序的權限。診斷只記錄錯誤碼與耗時，沒有輸出焦點內容或送出輸入。

同日以 `fixtures/SystemFocusAttributeDiagnostic.swift` 唯讀列出系統無障礙物件的屬性名稱。查詢成功，回報四個屬性，其中包含焦點 App 和焦點元素。這只能確認名稱可列出；此前讀取焦點值仍失敗，根因未知。診斷未輸出 App 身分或焦點內容，也沒有送出輸入。

同日以 `fixtures/SessionStatusDiagnostic.swift` 唯讀查詢目前的 Window Server 工作階段。字典可取得，登入完成和 on-console 均為 true；非公開鎖定欄位缺失，無法由此判定螢幕是否鎖定。這也無法解釋焦點查詢失敗。Apple 將 on-console 定義為工作階段是否位於主控台，並非解鎖證明。[Apple 文件](https://developer.apple.com/documentation/coregraphics/kcgsessiononconsolekey)。

同日查到 Keyhac 專案的 [2026-09-13 實測](https://github.com/crftwr/keyhac/blob/main/doc/dev/testing.md)：在其 macOS 26.6.2 測試環境中，未建立 `NSApplication` 的探測程序讀取系統焦點 App 時立即得到 -25204；建立 `NSApplication.sharedApplication()` 後，同一程序的查詢成功。該專案也記錄目標 App 忙碌時仍可能逾時。這是調查線索，不代表本機根因已確認，也不證明初始化 App 不改變焦點或不影響使用者。

`fixtures/NSApplicationFocusProbe.swift` 的 `--self-test` 和未帶參數都不初始化 App；前者使用合成資料，後者直接停止。`--run-probe` 才會週期取樣前景、游標及螢幕視窗順序；基準不穩或過期時停止。視窗編號只留在記憶體，不寫入結果。接著它讀取初始化前後的系統焦點狀態，並嘗試將自建 App 的啟用策略設為 prohibited。它不建立視窗，也不主動送輸入或呼叫啟用；初始化仍可能造成瞬間焦點變化。取樣可能漏掉瞬間變化，視窗清單也可能不完整；結果不代表使用者輸入未受影響。

2026-11-09 使用者同意後執行一次對照，沒有重試，也沒有送出輸入。系統焦點 App 在初始化前後都回報成功；因此此結果無法證明初始化造成改變。23 筆前景、游標和視窗順序取樣一致，但可能漏掉瞬間變化。`activation_policy_prohibited` 為 false；當時程式把設定回傳值和策略讀回合併成單一布林，因此無法區分是哪一步不符，也不能聲稱策略已禁止啟用。原始輸出轉錄（原始證據：來源專案 `benchmark/nsapp-focus-probe-20261109.json`，未隨本庫攜帶）

執行後的原始碼已改為另記初始化前 `NSApp` 是否存在、設定結果與實際策略值。這是為排查前一次的不確定性；新版只通過離線自測，尚未實際執行。重新實測仍須使用者事先同意，不得自動重試。

另以 `fixtures/PassiveNSAppDiagnostic.swift` 做一次唯讀檢查：在該程序讀取 `NSWorkspace.frontmostApplication` 前後，`NSApp` 均為 nil；前景 App 可取得，系統焦點 App 查詢回傳 -25204。此診斷沒有呼叫 `NSApplication.shared`、設定啟用策略或送出輸入。它與先前對照測試是不同程序，不能解釋為何先前初始化前查詢成功；仍需釐清系統狀態與程序差異。

2026-11-09 再以 `fixtures/PassiveFocusWarmupDiagnostic.swift` 在獨立程序做對照。三次讀取螢幕視窗清單後，系統焦點 App 與焦點元素均可取得，且元素程序與焦點 App 相同；對照、讀前景 App 和讀游標的三組各三次仍回傳 -25204。測試程序均沒有建立 `NSApplication`。這是本機少量樣本，不證明跨版本必然有效或輸入安全。對照結果（原始證據：來源專案 `benchmark/focus-warmup-20261109.json`，未隨本庫攜帶）

headless 實驗的唯讀 `--sample-only` 已先讀視窗清單，再讀系統焦點及取樣其他狀態。一次實測得到 15 筆取樣、零錯誤、觀察狀態一致，且 `NSApp` 不存在。沒有送出輸入。這只證明當次可以取得基準；事件投遞、焦點隔離和取樣間的變化仍未驗證。

後續加入前景 App、系統焦點 App 與焦點元素的交叉核對。一次唯讀取樣得到 14 筆有效資料但狀態不一致，程式回報未知。加入不含身分或座標的彙總診斷後，另一次得到 13 筆取樣、零錯誤且觀察狀態一致。沒有送出事件；這兩次不能推斷狀態變動的原因或保證未來輸入安全。

同日使用 `fixtures/PassiveActivationPolicyDiagnostic.swift` 唯讀檢查另一個獨立命令列程序。讀取前後 `NSApp` 均不存在；`NSRunningApplication.current.activationPolicy` 回傳原始值 -1，不等於 `.prohibited`。此值的原因尚未確認，也不代表其他程序或先前對照測試的策略；不得視為禁止啟用或輸入安全證據。沒有建立 App 或送出輸入。[Apple 屬性說明](https://developer.apple.com/documentation/appkit/nsrunningapplication/activationpolicy)
