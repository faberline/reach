# MCP stdio 包裝

`cu-mcp` 是一個 stdio 的 MCP 伺服器。每個工具呼叫都會轉成一筆 `cu-agent` 請求，交給一個新的 `cu-agent` 程序執行，再把它的回覆原樣放進工具結果。`cu-mcp` 不實作任何擷取、無障礙或判讀邏輯，也不送任何輸入；所有觀察與判斷仍由 `cu-agent` 與 Agent 負責。

## 評估結論

- **MCP 版本**：只支援 `2025-06-18`。用戶端要求其他版本時，仍回覆 `2025-06-18`，由用戶端決定是否繼續。
- **傳輸**：只支援 stdio。一行一筆 JSON-RPC 2.0 訊息，UTF-8，行內不含換行。
- **能力**：只有 `tools`。
- **不支援**的功能：HTTP 傳輸、JSON-RPC 批次（陣列）、resources、prompts、sampling、roots、elicitation、logging、completion、進度與取消通知、分頁、`listChanged`、`outputSchema`、圖片內容（只回傳文字與結構化 JSON）、任何輸入功能。原因：Reach 目前只提供唯讀的單次呼叫；沒有可訂閱的資源、長時間工作或輸入功能，所以這些能力沒有對應的後端，實作只會增加表面風險。

## 建置與啟動

```sh
cd <REACH_DIR>
cargo build --locked --bin cu-agent --bin cu-mcp
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' | target/debug/cu-mcp --cu-agent <REACH_DIR>/target/debug/cu-agent
```

第一個參數必須是 `--cu-agent` 加上絕對路徑。之後只能接 `--helper <欄位>=<絕對路徑>`，可以重複。其他參數、相對路徑、不是執行檔路徑的欄位，或重複指定同一個欄位，都會被拒絕（結束碼 2，stdout 沒有輸出）。`<REACH_DIR>` 是本儲存庫在呼叫者環境中的絕對路徑，使用時要換成實際路徑。

## 執行檔路徑在啟動時固定

`cu-agent` 會直接執行請求中的 helper 與 adapter 路徑。經由 MCP 時，工具參數由模型決定，所以 `cu-mcp` 不讓工具參數指定任何執行檔：

- **執行檔路徑欄位**是名稱以 `_helper` 結尾的欄位（例如 `ax_helper`、`capture_helper`、`ocr_helper`、`stream_helper`、`overlay_helper`、`notification_helper`、`session_helper`、`isolation_helper`），以及 `adapter` 與 `adapters`。
- 這些欄位只能在啟動時用 `--helper` 指定，例如 `--helper ax_helper=<REACH_DIR>/target/debug/mac-ax-snapshot`。`adapters` 可以重複 1 到 4 次，路徑不能重複；其他欄位只能指定一次。
- `tools/list` 不把這些欄位列為工具參數。`tools/call` 的參數帶了其中任何一個時，回報 `-32602`，不啟動 `cu-agent`。
- 方法需要的欄位已在啟動時指定時，`cu-mcp` 把固定的路徑加進請求再轉送。
- 方法需要的欄位沒有指定時，`tools/list` 不提供這個方法；仍然呼叫它時，結果是 `wrapper_error`，`reason` 會指出缺少的欄位，不啟動 `cu-agent`。
- `capture`、`capture_all_windows`、`accessibility`、`accessibility_subtree` 這四個在 `describe` 中帶有 `agent_fields` 的方法，經由 MCP 只走 ReachAgent App，不加 helper，也不受 `--helper` 影響。ReachAgent 沒有執行時，它們回報 `agent_unavailable`。
- `cu-mcp` 只檢查路徑是絕對路徑，不檢查檔案是否存在，也不驗證檔案內容。啟動 `cu-mcp` 的人必須自己確認這些執行檔可信。

`image` 與 `output` 這類輸出路徑仍然是工具參數。後果：模型可以要求在目前使用者有權寫入的任意絕對路徑建立新檔（`cu-agent` 不覆寫既有檔案），檔案內容可能包含截圖、視窗標題與程序路徑。`cu-mcp` 不限制輸出目錄；需要限制時，要由啟動 `cu-mcp` 的環境處理，例如用權限較低的帳號或沙箱執行。

## 用戶端設定範例

```json
{"mcpServers":{"reach":{"command":"<REACH_DIR>/target/debug/cu-mcp","args":["--cu-agent","<REACH_DIR>/target/debug/cu-agent","--helper","ax_helper=<REACH_DIR>/target/debug/mac-ax-snapshot"],"env":{"REACH_AGENT_SOCKET":"<HOME_DIR>/.reach/agent.sock"}}}}
```

`<REACH_DIR>` 與 `<HOME_DIR>` 是佔位路徑，要換成呼叫者環境中的絕對路徑。範例中的路徑不屬於任何特定機器，不要沿用。

## 工具與結果

- **工具清單**每次都由 `cu-agent` 的 `describe` 產生，`cu-mcp` 不保留備用清單。`describe` 失敗時，`tools/list` 回報錯誤 `-32603`，不會回傳空的或舊的清單。執行檔路徑欄位不在清單內，見上一節。
- **方法表**：`tools/call` 需要 `describe` 的方法表才知道要加哪些固定路徑。還沒有方法表時，第一次 `tools/call` 會先多啟動一次 `cu-agent` 執行 `describe`；之後每次呼叫只啟動一個 `cu-agent`。這次 `describe` 失敗時，結果是 `wrapper_error`，原本的請求不送出。
- **未知的工具名稱**不在 `cu-mcp` 判斷，照樣送給 `cu-agent`，由它回報 `unknown method`。
- **結構化結果** `structuredContent` 的欄位：
  - `action_allowed`：永遠是 `false`。`cu-mcp` 會檢查 `cu-agent` 回覆的每一層：最上層必須有 `action_allowed=false`，物件或陣列內任何層級只要出現不是 `false` 的 `action_allowed`，整個回覆都會被拒絕，並改為 `wrapper_error`。
  - `status`：`cu-agent` 回報的狀態，或 `wrapper_error`。
  - `needs_agent`：`status` 不是 `ok` 時為 `true`。
  - `exit_code`：`cu-agent` 的結束碼；無法取得時為 `null`。
  - `reply`：`cu-agent` 的完整回覆；`wrapper_error` 時為 `null`。
  - `reason`：只在 `wrapper_error` 時出現，說明原因。
- **`isError`**：`status` 是 `ok` 或 `unknown` 時為 `false`，其他情況為 `true`。
- **`needs_agent=true`** 時，結果必須交給 Agent 判斷。`cu-mcp` 不重試，也不降級到其他方法。
- **`action` 等輸入方法不經由 MCP 提供**：`tools/list` 不列出它們，`tools/call` 在執行 `cu-agent` 之前就以 `refused` 拒絕（不支援經由 MCP 輸入），不會送出任何事件。這個過濾與 ReachAgent 是否以 `--enable-input` 啟動無關；MCP 也不能啟用輸入。前景輸入只能由本機直接呼叫 `cu-agent`，見 [AGENT-SERVICE.md](AGENT-SERVICE.md) 的「輸入模式」。
- **`wrapper_error`** 表示 `cu-mcp` 無法取得有效的 `cu-agent` 回覆，例如程序無法啟動、逾時、回覆不是合法 JSON、回覆允許了操作，或方法需要的執行檔路徑沒有在啟動時指定。它不代表觀察成功或失敗的證據。

## 限制

- 請求行上限 65,536 bytes，超過的行會回報 `-32600`，伺服器繼續處理後面的行。`cu-agent` 自己的 65,536 bytes 上限仍然適用，被拒絕時原樣回報。
- 回覆上限 8 MiB。
- 每次呼叫上限 180 秒。逾時會終止 `cu-agent`，但它啟動的 helper 可能殘留。
- 不檢查 `initialize` 是否先送；用戶端應依 MCP 的順序使用。
- `--cu-agent` 與 `--helper` 指定的執行檔由啟動 `cu-mcp` 的人信任，`cu-mcp` 不驗證它們的來源。`cu-agent` 的介面見 [通用 Computer Use 的 JSON CLI](AGENT-CLI.md)。
- 非 macOS 上 `cu-agent` 只回報不支援，`cu-mcp` 會原樣轉述，不做額外判斷。
