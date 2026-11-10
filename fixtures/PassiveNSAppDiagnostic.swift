import AppKit
import ApplicationServices
import Foundation

let before = NSApp != nil
let beforeFrontAvailable = NSWorkspace.shared.frontmostApplication != nil
let after = NSApp != nil
let system = AXUIElementCreateSystemWide()
var raw: CFTypeRef?
let status = AXUIElementCopyAttributeValue(system, kAXFocusedApplicationAttribute as CFString, &raw)
let result: [String: Any] = ["nsapp_before_workspace":before, "nsapp_after_workspace":after,
    "frontmost_available":beforeFrontAvailable,"focused_app_status":status.rawValue,
    "input_posted":false,"action_allowed":false]
let data = try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys])
FileHandle.standardOutput.write(data); FileHandle.standardOutput.write(Data([10]))
