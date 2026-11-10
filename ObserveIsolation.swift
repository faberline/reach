// Read-only sampling. No events are posted, no app is activated, no windows are created.
import AppKit
import ApplicationServices
import Foundation

private enum Failure: Error { case badArgument, unavailable, frontOrCursorUnavailable, focusUnavailable, invalidWindowList }
private struct Sample {
    let started: TimeInterval
    let ended: TimeInterval
    let foreground: pid_t
    let focusPID: pid_t
    let focus: AXUIElement
    let focusScope: String
    let cursor: CGPoint
    let windowIDs: [Int]
}
private func sample() throws -> Sample {
    let start = ProcessInfo.processInfo.systemUptime
    guard let front = NSWorkspace.shared.frontmostApplication, front.processIdentifier > 0,
          let event = CGEvent(source: nil), event.location.x.isFinite, event.location.y.isFinite else { throw Failure.frontOrCursorUnavailable }
    let system = AXUIElementCreateSystemWide()
    _ = AXUIElementSetMessagingTimeout(system, 0.2)
    var raw: CFTypeRef?
    let systemError = AXUIElementCopyAttributeValue(system, kAXFocusedUIElementAttribute as CFString, &raw)
    var scope = "system"
    if systemError != .success || raw == nil {
        let app = AXUIElementCreateApplication(front.processIdentifier)
        _ = AXUIElementSetMessagingTimeout(app, 0.2)
        raw = nil
        guard AXUIElementCopyAttributeValue(app, kAXFocusedUIElementAttribute as CFString, &raw) == .success else { throw Failure.focusUnavailable }
        scope = "front_app_only"
    }
    guard let value=raw, CFGetTypeID(value)==AXUIElementGetTypeID() else { throw Failure.focusUnavailable }
    let focus=unsafeBitCast(value, to: AXUIElement.self)
    var pid:pid_t=0
    guard AXUIElementGetPid(focus, &pid) == .success, pid>0 else { throw Failure.focusUnavailable }
    guard let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String:Any]],
          !windows.isEmpty, windows.count<=4096 else { throw Failure.invalidWindowList }
    var ids:[Int]=[]
    for window in windows {
        guard let id=window[kCGWindowNumber as String] as? Int, id>0 else { throw Failure.invalidWindowList }
        ids.append(id)
    }
    guard Set(ids).count==ids.count else { throw Failure.invalidWindowList }
    return Sample(started:start,ended:ProcessInfo.processInfo.systemUptime,foreground:front.processIdentifier,focusPID:pid,focus:focus,focusScope:scope,cursor:event.location,windowIDs:ids)
}
private func run() throws -> [String:Any] {
    guard CommandLine.arguments == [CommandLine.arguments[0], "--sample"] else { throw Failure.badArgument }
    var values:[Sample]=[]
    for index in 0..<10 {
        if index>0 { Thread.sleep(forTimeInterval:0.05) }
        values.append(try sample())
    }
    guard let first=values.first, let last=values.last else { throw Failure.unavailable }
    let consistentScope = values.allSatisfy{ $0.focusScope == first.focusScope }
    let focusUnchanged: Any = consistentScope ? (values.allSatisfy{ $0.focusPID==first.focusPID && CFEqual($0.focus,first.focus) } as Any) : NSNull()
    let frontUnchanged=values.allSatisfy{ $0.foreground==first.foreground }
    let pointerUnchanged=values.allSatisfy{ $0.cursor==first.cursor }
    let orderUnchanged=values.allSatisfy{ $0.windowIDs==first.windowIDs }
    let longest=values.map{$0.ended-$0.started}.max() ?? 0
    let gaps=zip(values.dropFirst(), values).map { $0.started - $1.ended }
    let longestGap = gaps.max() ?? 0
    let timingOK = values.allSatisfy { $0.started.isFinite && $0.ended.isFinite && $0.started >= 0 && $0.ended >= $0.started && $0.ended - $0.started <= 0.25 } && gaps.allSatisfy { $0.isFinite && $0 >= 0 && $0 <= 0.25 }
    let systemSampled = first.focusScope == "system" && consistentScope
    return ["status": systemSampled && timingOK ? "sampled" : "sampled_partial", "focus_scope": consistentScope ? first.focusScope : "mixed", "system_focus_sampled": systemSampled, "timing_within_limit":timingOK, "action_allowed":false,"samples":values.count,
            "duration_seconds":last.ended-first.started, "longest_sample_seconds":longest, "longest_gap_seconds":longestGap,
            "foreground_unchanged":frontUnchanged, "focus_unchanged":focusUnchanged,
            "cursor_unchanged":pointerUnchanged, "window_order_unchanged":orderUnchanged,
            "input_posted":false, "note":"Read-only baseline; sampling can miss transient changes. No input isolation or event delivery was verified."]
}
do {
    let data=try JSONSerialization.data(withJSONObject:run(),options:[.sortedKeys]);FileHandle.standardOutput.write(data);FileHandle.standardOutput.write(Data([10]))
} catch {
    let data=try! JSONSerialization.data(withJSONObject:["status":"unknown","action_allowed":false,"input_posted":false,"reason":String(describing:error)],options:[.sortedKeys]);FileHandle.standardOutput.write(data);FileHandle.standardOutput.write(Data([10]));exit(1)
}
