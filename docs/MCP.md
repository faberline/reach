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

`--cu-agent` 之後必須是絕對路徑，其他參數都會被拒絕（結束碼 2，stdout 沒有輸出）。`<REACH_DIR>` 是本儲存庫在呼叫者環境中的絕對路徑，使用時要換成實際路徑。

## 用戶端設定範例

```json
{"mcpServers":{"reach":{"command":"<REACH_DIR>/target/debug/cu-mcp","args":["--cu-agent","<REACH_DIR>/target/debug/cu-agent"],"env":{"REACH_AGENT_SOCKET":"<HOME_DIR>/.reach/agent.sock"}}}}
```

`<REACH_DIR>` 與 `<HOME_DIR>` 是佔位路徑，要換成呼叫者環境中的絕對路徑。範例中的路徑不屬於任何特定機器，不要沿用。

## 工具與結果

- **工具清單**每次都由 `cu-agent` 的 `describe` 產生，`cu-mcp` 不保留備用清單。`describe` 失敗時，`tools/list` 回報錯誤 `-32603`，不會回傳空的或舊的清單。
- **未知的工具名稱**不在 `cu-mcp` 判斷，照樣送給 `cu-agent`，由它回報 `unknown method`。
- **結構化結果** `structuredContent` 的欄位：
  - `action_allowed`：永遠是 `false`。`cu-mcp` 會拒絕任何 `action_allowed` 不是 `false` 的回覆，並改為 `wrapper_error`。
  - `status`：`cu-agent` 回報的狀態，或 `wrapper_error`。
  - `needs_agent`：`status` 不是 `ok` 時為 `true`。
  - `exit_code`：`cu-agent` 的結束碼；無法取得時為 `null`。
  - `reply`：`cu-agent` 的完整回覆；`wrapper_error` 時為 `null`。
  - `reason`：只在 `wrapper_error` 時出現，說明原因。
- **`isError`**：`status` 是 `ok` 或 `unknown` 時為 `false`，其他情況為 `true`。
- **`needs_agent=true`** 時，結果必須交給 Agent 判斷。`cu-mcp` 不重試，也不降級到其他方法。
- **`action`** 經由 MCP 仍回報 `unsupported`（不支援），不會送出任何事件。
- **`wrapper_error`** 表示 `cu-mcp` 無法取得有效的 `cu-agent` 回覆，例如程序無法啟動、逾時、回覆不是合法 JSON，或回覆允許了操作。它不代表觀察成功或失敗的證據。

## 限制

- 請求行上限 65,536 bytes，超過的行會回報 `-32600`，伺服器繼續處理後面的行。`cu-agent` 自己的 65,536 bytes 上限仍然適用，被拒絕時原樣回報。
- 回覆上限 8 MiB。
- 每次呼叫上限 180 秒。逾時會終止 `cu-agent`，但它啟動的 helper 可能殘留。
- 不檢查 `initialize` 是否先送；用戶端應依 MCP 的順序使用。
- helper 路徑仍由呼叫端信任，`cu-mcp` 不驗證 `cu-agent` 的來源。`cu-agent` 的介面見 [通用 Computer Use 的 JSON CLI](AGENT-CLI.md)。
- 非 macOS 上 `cu-agent` 只回報不支援，`cu-mcp` 會原樣轉述，不做額外判斷。
