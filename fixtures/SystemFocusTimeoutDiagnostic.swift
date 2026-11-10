import ApplicationServices
import Foundation

var rows: [[String: Any]] = []
for timeout: Float in [0.1, 0.5, 1.5] {
    let system = AXUIElementCreateSystemWide()
    let setStatus = AXUIElementSetMessagingTimeout(system, timeout)
    var counts: [String: Int] = [:]
    var durations: [Double] = []
    for _ in 0..<3 {
        var value: CFTypeRef?
        let start = ProcessInfo.processInfo.systemUptime
        let status = AXUIElementCopyAttributeValue(system, kAXFocusedApplicationAttribute as CFString, &value)
        durations.append(ProcessInfo.processInfo.systemUptime - start)
        counts[String(status.rawValue), default: 0] += 1
    }
    rows.append(["timeout_seconds":timeout, "set_status":setStatus.rawValue, "status_counts":counts, "elapsed_seconds":durations])
}
let data = try JSONSerialization.data(withJSONObject: ["trusted":AXIsProcessTrusted(), "results":rows, "input_posted":false, "action_allowed":false], options: [.sortedKeys])
FileHandle.standardOutput.write(data)
FileHandle.standardOutput.write(Data([10]))
