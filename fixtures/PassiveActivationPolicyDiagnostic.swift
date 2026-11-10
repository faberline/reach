import AppKit
import Foundation

let before = NSApp != nil
let app = NSRunningApplication.current
let policy = app.activationPolicy
let after = NSApp != nil
let body: [String: Any] = ["nsapp_before":before,"nsapp_after":after,
    "activation_policy_raw":policy.rawValue,"prohibited":policy == .prohibited,
    "input_posted":false,"action_allowed":false]
let data = try JSONSerialization.data(withJSONObject: body, options: [.sortedKeys])
FileHandle.standardOutput.write(data); FileHandle.standardOutput.write(Data([10]))
