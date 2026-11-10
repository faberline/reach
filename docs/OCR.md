# 離線畫面文字辨識

此功能讀取已保存的 PNG。它不連接視窗，也不操作 App。文字可能錯誤或遺漏；辨識信心未校準。框座標是圖片左下角原點的 0 至 1 比例，不是視窗或螢幕座標，不能用來點擊。

macOS 需要 Vision、CryptoKit、Swift 和相應語言資料。一般沙箱曾無法執行 Vision；授權環境可以。輔助程式與圖片由呼叫端指定，只能使用可信的輔助程式。輸出可能含畫面中的私人資料；不要自動上傳。

```sh
# 在儲存庫根目錄執行
mkdir -p /private/tmp/sw-ocr-bin
swiftc -module-cache-path /private/tmp/sw-computer-use-swift-cache RecognizeText.swift -o /private/tmp/sw-ocr-bin/recognize-text
CARGO_HOME=/private/tmp/sw-cargo-cache cargo build --locked --offline --bin cu-agent
printf '%s\n' '{"id":"ocr-1","method":"recognize_text","image":"/absolute/existing.png","ocr_helper":"/private/tmp/sw-ocr-bin/recognize-text"}' | target/debug/cu-agent
```

呼叫端須換成絕對 PNG 路徑。程式檢查前後圖片雜湊、尺寸和回覆安全欄位；不證明圖片取自指定視窗，也不能阻止檢查之間的瞬間替換。OCR 完成回傳 `unknown`、狀態碼 3 和 `action_allowed=false`。輔助程式失敗、逾時或圖片不符回傳錯誤。該 helper 為可信輸入，不驗證它的來源；逾時的清理限制見 AGENT-CLI.md。

2026-11-08：以火山連刷停止畫面測試，能讀出「由於戰鬥失敗，已結束連續戰鬥。」同張圖片也有文字誤讀。這只是單張開發樣本，不是準確率或可安全操作的證據。

2026-11-09：Rust 逐筆檢查文字、未校準信心值和圖片內的正規化範圍；缺欄位、超出範圍或格式錯誤會拒絕整份 OCR 結果。這只檢查格式，不保證辨識正確或授權點擊。對既有遊戲截圖執行一次唯讀測試，33 筆結果通過格式檢查；這不是準確率測試。
