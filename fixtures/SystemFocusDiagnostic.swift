import AppKit
import ApplicationServices
import Foundation

let system = AXUIElementCreateSystemWide()
_ = AXUIElementSetMessagingTimeout(system, 0.2)
var counts: [String: Int] = [:]
for _ in 0..<10 {
    var appValue: CFTypeRef?
    let appStatus = AXUIElementCopyAttributeValue(system, kAXFocusedApplicationAttribute as CFString, &appValue)
    var code = "application_error_\(appStatus.rawValue)"
    if appStatus == .success, let value = appValue, CFGetTypeID(value) == AXUIElementGetTypeID() {
        let app = unsafeBitCast(value, to: AXUIElement.self)
        _ = AXUIElementSetMessagingTimeout(app, 0.2)
        var appPID: pid_t = 0
        let pidStatus = AXUIElementGetPid(app, &appPID)
        var focusedValue: CFTypeRef?
        let focusStatus = AXUIElementCopyAttributeValue(app, kAXFocusedUIElementAttribute as CFString, &focusedValue)
        if pidStatus == .success, appPID > 0, focusStatus == .success, let focused = focusedValue,
           CFGetTypeID(focused) == AXUIElementGetTypeID() {
            let element = unsafeBitCast(focused, to: AXUIElement.self)
            var elementPID: pid_t = 0
            let elementStatus = AXUIElementGetPid(element, &elementPID)
            code = elementStatus == .success && elementPID == appPID ? "anchored_focus_ok" : "element_pid_mismatch_or_error"
        } else { code = "app_focus_error_\(focusStatus.rawValue)" }
    }
    counts[code, default: 0] += 1
    Thread.sleep(forTimeInterval: 0.05)
}
let data = try JSONSerialization.data(withJSONObject: ["counts":counts, "action_allowed":false, "input_posted":false], options: [.sortedKeys])
FileHandle.standardOutput.write(data)
FileHandle.standardOutput.write(Data([10]))
