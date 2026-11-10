import ApplicationServices
import Foundation

let system = AXUIElementCreateSystemWide()
var attributes: CFArray?
let status = AXUIElementCopyAttributeNames(system, &attributes)
var count = 0
var focusedApp = false
var focusedElement = false
if status == .success, let array = attributes as? [String] {
    count = array.count
    focusedApp = array.contains(kAXFocusedApplicationAttribute as String)
    focusedElement = array.contains(kAXFocusedUIElementAttribute as String)
}
let body: [String: Any] = ["status_code":status.rawValue, "attribute_count":count,
    "focused_app_listed":focusedApp, "focused_element_listed":focusedElement,
    "trusted":AXIsProcessTrusted(), "input_posted":false, "action_allowed":false]
let data = try JSONSerialization.data(withJSONObject: body, options: [.sortedKeys])
FileHandle.standardOutput.write(data)
FileHandle.standardOutput.write(Data([10]))
