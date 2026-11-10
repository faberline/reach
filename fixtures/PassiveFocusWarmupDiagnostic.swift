import AppKit
import ApplicationServices
import Foundation

let args = Array(CommandLine.arguments.dropFirst())
guard args.count == 1, ["control", "workspace", "cursor", "windows"].contains(args[0]) else { exit(2) }
func focus() -> (code: Int32, valid: Bool, pid: pid_t?) {
    let system = AXUIElementCreateSystemWide()
    _ = AXUIElementSetMessagingTimeout(system, 0.2)
    var value: CFTypeRef?
    let status = AXUIElementCopyAttributeValue(system, kAXFocusedApplicationAttribute as CFString, &value)
    guard status == .success, let value, CFGetTypeID(value) == AXUIElementGetTypeID() else { return (status.rawValue, false, nil) }
    let element = unsafeBitCast(value, to: AXUIElement.self)
    var pid: pid_t = 0
    let pidStatus = AXUIElementGetPid(element, &pid)
    return (status.rawValue, true, pidStatus == .success && pid > 0 ? pid : nil)
}
let before = focus()
var readSucceeded = true
switch args[0] {
case "workspace": readSucceeded = NSWorkspace.shared.frontmostApplication != nil
case "cursor": readSucceeded = CGEvent(source: nil) != nil
case "windows": readSucceeded = CGWindowListCopyWindowInfo(.optionOnScreenOnly, kCGNullWindowID) != nil
default: break
}
let after = focus()
let system = AXUIElementCreateSystemWide()
var focusedValue: CFTypeRef?
let elementStatus = AXUIElementCopyAttributeValue(system, kAXFocusedUIElementAttribute as CFString, &focusedValue)
var elementPID: pid_t = 0
var elementValid = false
if elementStatus == .success, let value = focusedValue, CFGetTypeID(value) == AXUIElementGetTypeID() {
    let element = unsafeBitCast(value, to: AXUIElement.self)
    elementValid = AXUIElementGetPid(element, &elementPID) == .success && elementPID > 0
}
let front = NSWorkspace.shared.frontmostApplication?.processIdentifier
let result: [String: Any] = ["mode":args[0],"before":before.code,"after":after.code,"after_valid":after.valid,
    "after_matches_frontmost":after.pid != nil && after.pid == front,"read_succeeded":readSucceeded,
    "focused_element_status":elementStatus.rawValue,"focused_element_valid":elementValid,
    "focused_element_matches_app":elementValid && after.pid == elementPID,
    "nsapp_present":NSApp != nil,"input_posted":false,"action_allowed":false]
let data = try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys])
FileHandle.standardOutput.write(data); FileHandle.standardOutput.write(Data([10]))
