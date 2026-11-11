// Accessibility snapshots for the ReachAgent service (faberline/reach-old#39). It is compiled together with
// ReachAgent.swift. Read-only AX: no input events, no actions, no foreground change.
// It never asks for a permission: the service pre-flights `accessibility` before it calls a
// backend, so a backend runs only when the permission is already granted.
// The code below is ported from AccessibilitySnapshot.swift (the one-shot helper) without its
// own permission guard and command line. The ported `snapshot` and `subtree` are named
// `axSnapshot` and `axSubtree` here, so that the backend methods of the same name call them.
import AppKit
import ApplicationServices
import Darwin
import Foundation

private let maxDepth = 6
private let maxNodes = 160
private let maxString = 240
private let maxActions = 32
private let maxWindows = 32
private let snapshotSeconds = 5.0

/// A failed accessibility read: `status` is the stable code, `reason` the readable detail.
struct AccessibilityFailure: Error, CustomStringConvertible {
    let status: String
    let reason: String
    var description: String { "\(status): \(reason)" }
    init(_ status: String, _ reason: String) {
        self.status = status
        self.reason = reason
    }
}

/// Contract: read the process's AX tree without any side effect and return the report, or throw
/// `AccessibilityFailure`. The service calls a backend only after the `accessibility`
/// permission pre-flight.
protocol AccessibilityBackend: Sendable {
    func snapshot(pid: Int32, windowsOnly: Bool) throws -> [String: Any]
    func subtree(pid: Int32, source: String, path: [Int]) throws -> [String: Any]
}

private enum SnapshotFailure: Error, CustomStringConvertible {
    case invalidArguments, processUnavailable, processChanged
    case windowsUnavailable(Int32), invalidWindowsType, subtreeUnavailable
    var description: String {
        switch self {
        case .invalidArguments: return "expected a positive process ID"
        case .processUnavailable: return "process is not a running application or its start time is unavailable"
        case .processChanged: return "process identity changed during snapshot"
        case .windowsUnavailable(let code): return "cannot read AX windows (AXError \(code))"
        case .invalidWindowsType: return "AX windows did not contain an array of AX elements"
        case .subtreeUnavailable: return "AX subtree unavailable or path invalid"
        }
    }
    var code: String {
        switch self {
        case .invalidArguments: return "invalid_arguments"
        case .processUnavailable: return "process_unavailable"
        case .processChanged: return "process_changed"
        case .windowsUnavailable: return "ax_windows_unavailable"
        case .invalidWindowsType: return "ax_windows_invalid_type"
        case .subtreeUnavailable: return "ax_subtree_unavailable"
        }
    }
}

private func limited(_ value: String) -> (text: String, truncated: Bool) {
    let prefix = String(value.prefix(maxString))
    return (prefix, value.count > maxString)
}

// Use the same kernel source in discovery, identity and AX snapshots. AppKit's
// launchDate is nil for some running apps, including Finder on this host.
func kernelStart(_ pid: pid_t) -> Double? {
    guard pid > 0 else { return nil }
    var mib = [CTL_KERN, KERN_PROC, KERN_PROC_PID, pid]
    var info = kinfo_proc()
    var size = MemoryLayout<kinfo_proc>.size
    guard sysctl(&mib, 4, &info, &size, nil, 0) == 0,
          size == MemoryLayout<kinfo_proc>.size, info.kp_proc.p_pid == pid else { return nil }
    let start = info.kp_proc.p_starttime
    guard start.tv_sec > 0, (0..<1_000_000).contains(start.tv_usec) else { return nil }
    return Double(start.tv_sec) + Double(start.tv_usec) / 1_000_000
}

private final class AXBudget {
    let deadline = ProcessInfo.processInfo.systemUptime + snapshotSeconds
    var count = 0
    var truncated = false
    func available() -> Bool {
        guard count < maxNodes, ProcessInfo.processInfo.systemUptime < deadline else {
            truncated = true
            return false
        }
        return true
    }
}

private func markTextTruncated(_ result: inout [String: Any], field: String, truncated: Bool, budget: AXBudget) {
    if truncated { result["\(field)_truncated"] = true; budget.truncated = true }
}

private func recordActions(_ names: [String], result: inout [String: Any], budget: AXBudget) {
    let limitedNames = names.prefix(maxActions).map(limited)
    result["actions"] = limitedNames.map(\.text)
    let truncatedIndices = limitedNames.enumerated().compactMap { $0.element.truncated ? $0.offset : nil }
    if !truncatedIndices.isEmpty { result["action_names_truncated_indices"] = truncatedIndices; budget.truncated = true }
    if names.count > maxActions { result["actions_truncated"] = true; budget.truncated = true }
}

private func attr(_ element: AXUIElement, _ name: CFString, _ budget: AXBudget) -> (CFTypeRef?, Int32?) {
    guard budget.available() else { return (nil, nil) }
    var result: CFTypeRef?
    let status = AXUIElementCopyAttributeValue(element, name, &result)
    return status == .success ? (result, nil) : (nil, status.rawValue)
}
private func stringAttr(_ element: AXUIElement, _ name: CFString, _ budget: AXBudget) -> (String?, Int32?, Bool) {
    let (value, error) = attr(element, name, budget)
    guard let value, CFGetTypeID(value) == CFStringGetTypeID() else { return (nil, error, false) }
    let result = limited(value as! String)
    return (result.text, nil, result.truncated)
}
private func elements(_ value: CFTypeRef?, limit: Int = maxNodes) -> ([AXUIElement], Int)? {
    guard let value, CFGetTypeID(value) == CFArrayGetTypeID(), let values = value as? [AnyObject] else { return nil }
    var result: [AXUIElement] = []
    for item in values.prefix(limit) {
        guard CFGetTypeID(item) == AXUIElementGetTypeID() else { return nil }
        result.append(unsafeBitCast(item, to: AXUIElement.self))
    }
    return (result, values.count)
}
private func pointValue(_ value: CFTypeRef?) -> CGPoint? {
    guard let value, CFGetTypeID(value) == AXValueGetTypeID() else { return nil }
    let ax = unsafeBitCast(value, to: AXValue.self)
    guard AXValueGetType(ax) == .cgPoint else { return nil }
    var p = CGPoint.zero
    guard AXValueGetValue(ax, .cgPoint, &p), p.x.isFinite, p.y.isFinite else { return nil }
    return p
}
private func sizeValue(_ value: CFTypeRef?) -> CGSize? {
    guard let value, CFGetTypeID(value) == AXValueGetTypeID() else { return nil }
    let ax = unsafeBitCast(value, to: AXValue.self)
    guard AXValueGetType(ax) == .cgSize else { return nil }
    var s = CGSize.zero
    guard AXValueGetValue(ax, .cgSize, &s), s.width.isFinite, s.height.isFinite, s.width > 0, s.height > 0 else { return nil }
    return s
}
private func node(_ element: AXUIElement, path: [Int], depth: Int, ownerPID: pid_t, budget: AXBudget, windowsOnly: Bool = false) -> [String: Any]? {
    guard budget.available() else { return nil }
    budget.count += 1
    // Per-element timeout only limits AX messaging; it does not mutate the target.
    _ = AXUIElementSetMessagingTimeout(element, 0.2)
    var actualPID: pid_t = 0
    let ownerStatus = AXUIElementGetPid(element, &actualPID)
    guard ownerStatus == .success, actualPID == ownerPID else {
        // Embedded elements owned by other processes are outside the PID scope.
        return ["path": path, "status": ownerStatus == .success ? "owner_mismatch" : "owner_unverified",
                "children": NSNull(), "owner_error": ownerStatus.rawValue,
                "owner_pid": ownerStatus == .success ? actualPID as Any : NSNull()]
    }
    let (role, roleError, roleTruncated) = stringAttr(element, kAXRoleAttribute as CFString, budget)
    let (title, titleError, titleTruncated) = stringAttr(element, kAXTitleAttribute as CFString, budget)
    let (position, _) = attr(element, kAXPositionAttribute as CFString, budget)
    let (size, _) = attr(element, kAXSizeAttribute as CFString, budget)
    var result: [String: Any] = ["path": path, "owner_pid": actualPID, "role": role as Any? ?? NSNull(),
                                 "title": title as Any? ?? NSNull(), "actions": NSNull(),
                                 "frame": NSNull(), "children": []]
    if let roleError { result["role_error"] = roleError }
    if let titleError { result["title_error"] = titleError }
    markTextTruncated(&result, field: "role", truncated: roleTruncated, budget: budget)
    markTextTruncated(&result, field: "title", truncated: titleTruncated, budget: budget)
    if let p = pointValue(position), let s = sizeValue(size) {
        result["frame"] = ["x": p.x, "y": p.y, "width": s.width, "height": s.height]
    }
    if windowsOnly {
        result["children"] = NSNull()
        result["children_not_requested"] = true
        return result
    }
    if budget.available() {
        var raw: CFArray?
        let status = AXUIElementCopyActionNames(element, &raw)
        if status == .success, let names = raw as? [String] {
            recordActions(names, result: &result, budget: budget)
        } else { result["actions_error"] = status == .success ? "invalid_type" as Any : status.rawValue }
    }
    if depth < maxDepth, budget.available() {
        let (raw, error) = attr(element, kAXChildrenAttribute as CFString, budget)
        if let (children, childCount) = elements(raw, limit: maxNodes - budget.count) {
            var items: [[String: Any]] = []
            for (index, child) in children.enumerated() {
                guard let item = node(child, path: path + [index], depth: depth + 1, ownerPID: ownerPID, budget: budget) else { break }
                items.append(item)
            }
            result["children"] = items
            if childCount > items.count { result["children_truncated"] = true; budget.truncated = true }
        } else if let error { result["children_error"] = error }
        else if raw != nil { result["children_error"] = "invalid_type" }
    } else if depth >= maxDepth { result["children_truncated"] = true; budget.truncated = true }
    return result
}

private func addFallback(_ element: AXUIElement, source: String, to candidates: inout [(element: AXUIElement, sources: [String])]) {
    if let index = candidates.firstIndex(where: { CFEqual($0.element, element) }) {
        candidates[index].sources.append(source)
    } else { candidates.append((element, [source])) }
}

private func axSubtree(pid: pid_t, source: String, indexes: [Int]) throws -> [String: Any] {
    guard NSRunningApplication(processIdentifier: pid) != nil, let start = kernelStart(pid),
          indexes.count <= 8, indexes.allSatisfy({ $0 >= 0 && $0 < maxNodes }) else { throw SnapshotFailure.subtreeUnavailable }
    let root = AXUIElementCreateApplication(pid), budget = AXBudget()
    _ = AXUIElementSetMessagingTimeout(root, 0.2)
    let currentRoot: AXUIElement
    let childIndexes: ArraySlice<Int>
    if source == "windows" {
        guard let index = indexes.first, index < maxWindows else { throw SnapshotFailure.invalidArguments }
        let (raw, _) = attr(root, kAXWindowsAttribute as CFString, budget)
        guard let (windows, count) = elements(raw, limit: maxWindows), index < count, index < windows.count else { throw SnapshotFailure.subtreeUnavailable }
        currentRoot = windows[index]; childIndexes = indexes.dropFirst()
    } else {
        let rootAttribute: String
        switch source { case "main": rootAttribute = kAXMainWindowAttribute; case "focused": rootAttribute = kAXFocusedWindowAttribute
        default: throw SnapshotFailure.invalidArguments }
        let (value, _) = attr(root, rootAttribute as CFString, budget)
        guard let value, CFGetTypeID(value) == AXUIElementGetTypeID() else { throw SnapshotFailure.subtreeUnavailable }
        currentRoot = unsafeBitCast(value, to: AXUIElement.self); childIndexes = indexes[...]
    }
    var current = currentRoot
    for index in childIndexes {
        var owner: pid_t = 0
        guard AXUIElementGetPid(current, &owner) == .success, owner == pid else { throw SnapshotFailure.subtreeUnavailable }
        _ = AXUIElementSetMessagingTimeout(current, 0.2)
        let (raw, _) = attr(current, kAXChildrenAttribute as CFString, budget)
        guard let (children, count) = elements(raw, limit: maxNodes), index < count, index < children.count else { throw SnapshotFailure.subtreeUnavailable }
        current = children[index]
    }
    guard let item = node(current, path: indexes, depth: 0, ownerPID: pid, budget: budget, windowsOnly: false),
          item["status"] == nil, NSRunningApplication(processIdentifier: pid) != nil, kernelStart(pid) == start else { throw SnapshotFailure.subtreeUnavailable }
    return ["schema_version":1,"status":"ok","pid":pid,"process_start_unix_seconds":start, "process_start_source":"kernel_sysctl_kern_proc",
            "source":source,"path":indexes,"node":item,"node_count":budget.count,"truncated":budget.truncated,
            "scope":"read_only_subtree_unverified","cg_window_binding":"unverified","action_allowed":false]
}

private func axSnapshot(pid: pid_t, windowsOnly: Bool = false) throws -> [String: Any] {
    guard let app = NSRunningApplication(processIdentifier: pid), let start = kernelStart(pid) else { throw SnapshotFailure.processUnavailable }
    let root = AXUIElementCreateApplication(pid)
    _ = AXUIElementSetMessagingTimeout(root, 0.2)
    var raw: CFTypeRef?
    let status = AXUIElementCopyAttributeValue(root, kAXWindowsAttribute as CFString, &raw)
    guard status == .success else { throw SnapshotFailure.windowsUnavailable(status.rawValue) }
    guard let (windows, windowCount) = elements(raw, limit: maxWindows) else { throw SnapshotFailure.invalidWindowsType }
    let budget = AXBudget()
    var items: [[String: Any]] = []
    for (index, window) in windows.enumerated() {
        guard var item = node(window, path: [index], depth: 0, ownerPID: pid, budget: budget, windowsOnly: windowsOnly) else { break }
        item["index"] = index
        items.append(item)
    }
    if windowCount > items.count { budget.truncated = true }
    // Some apps report an empty AXWindows array but expose a main or focused
    // window. These are separate, incomplete hints, never a replacement list.
    var fallback: [String: Any]? = nil
    if windowCount == 0 && !windowsOnly {
        var candidates: [(element: AXUIElement, sources: [String])] = []
        var diagnostics: [String: Any] = [:]
        for (source, name) in [("main", kAXMainWindowAttribute), ("focused", kAXFocusedWindowAttribute)] {
            let (raw, error) = attr(root, name as CFString, budget)
            if let error { diagnostics[source] = error; continue }
            guard let raw, CFGetTypeID(raw) == AXUIElementGetTypeID() else {
                diagnostics[source] = "missing_or_invalid_type"; continue
            }
            let element = unsafeBitCast(raw, to: AXUIElement.self)
            addFallback(element, source: source, to: &candidates)
            diagnostics[source] = "read"
        }
        var rows: [[String: Any]] = []
        for (index, candidate) in candidates.enumerated() {
            guard var item = node(candidate.element, path: [index], depth: 0, ownerPID: pid, budget: budget, windowsOnly: false) else { break }
            item["sources"] = candidate.sources
            rows.append(item)
        }
        if rows.count < candidates.count { budget.truncated = true }
        fallback = ["scope":"main_or_focused_incomplete", "diagnostics":diagnostics,
                    "candidates":rows, "candidate_count_reported":candidates.count,
                    "complete_window_list":false, "cg_window_binding":"unverified", "action_allowed":false]
    }
    guard NSRunningApplication(processIdentifier: pid) != nil, kernelStart(pid) == start else { throw SnapshotFailure.processChanged }
    return ["schema_version": 1, "status": "ok", "pid": pid,
            "process_start_unix_seconds": start, "process_start_source":"kernel_sysctl_kern_proc",
            "bundle_id": app.bundleIdentifier as Any? ?? NSNull(),
            "executable": app.executableURL?.path as Any? ?? NSNull(),
            "scope": "process_ax_windows", "tree_scope": windowsOnly ? "windows_only" : "full_bounded_tree", "cg_window_binding": "unverified",
            "window_count_reported": windowCount, "node_count": budget.count,
            "truncated": budget.truncated, "action_allowed": false, "windows": items,
            "fallback": fallback as Any? ?? NSNull()]
}

// MARK: - System backend

/// Reads the real AX tree. Real-machine behavior is not verified yet: it needs the Accessibility
/// grant of the signed ReachAgent.app (issues #5 and #6).
struct SystemAccessibilityBackend: AccessibilityBackend {
    func snapshot(pid: Int32, windowsOnly: Bool) throws -> [String: Any] {
        do { return try axSnapshot(pid: pid, windowsOnly: windowsOnly) }
        catch let failure as SnapshotFailure { throw AccessibilityFailure(failure.code, failure.description) }
    }

    func subtree(pid: Int32, source: String, path: [Int]) throws -> [String: Any] {
        do { return try axSubtree(pid: pid, source: source, indexes: path) }
        catch let failure as SnapshotFailure { throw AccessibilityFailure(failure.code, failure.description) }
    }
}

// MARK: - Fake backend (test mode and self-test)

/// Deterministic reports without any AX read. pid 4242 is "not running"; a subtree path that
/// contains the index 99 is "unavailable".
struct FakeAccessibilityBackend: AccessibilityBackend {
    func snapshot(pid: Int32, windowsOnly: Bool) throws -> [String: Any] {
        guard pid != 4242 else {
            throw AccessibilityFailure("process_unavailable", SnapshotFailure.processUnavailable.description)
        }
        let window: [String: Any] = [
            "path": [0], "index": 0, "owner_pid": pid, "role": "AXWindow", "title": "Fake",
            "actions": [] as [String],
            "frame": ["x": 0.0, "y": 0.0, "width": 2.0, "height": 2.0],
            "children": (windowsOnly ? NSNull() : [[String: Any]]()) as Any,
        ]
        var reported = window
        if windowsOnly { reported["children_not_requested"] = true }
        return ["schema_version": 1, "status": "ok", "pid": pid,
                "process_start_unix_seconds": 1.0, "process_start_source": "kernel_sysctl_kern_proc",
                "bundle_id": NSNull(), "executable": NSNull(),
                "scope": "process_ax_windows", "tree_scope": windowsOnly ? "windows_only" : "full_bounded_tree",
                "cg_window_binding": "unverified", "window_count_reported": 1, "node_count": 1,
                "truncated": false, "action_allowed": false, "windows": [reported], "fallback": NSNull()]
    }

    func subtree(pid: Int32, source: String, path: [Int]) throws -> [String: Any] {
        guard pid != 4242 else {
            throw AccessibilityFailure("process_unavailable", SnapshotFailure.processUnavailable.description)
        }
        guard !path.contains(99) else {
            throw AccessibilityFailure("ax_subtree_unavailable", SnapshotFailure.subtreeUnavailable.description)
        }
        let item: [String: Any] = [
            "path": path, "owner_pid": pid, "role": "AXGroup", "title": NSNull(),
            "actions": [] as [String], "frame": NSNull(), "children": [] as [[String: Any]],
        ]
        return ["schema_version": 1, "status": "ok", "pid": pid,
                "process_start_unix_seconds": 1.0, "process_start_source": "kernel_sysctl_kern_proc",
                "source": source, "path": path, "node": item, "node_count": 1, "truncated": false,
                "scope": "read_only_subtree_unverified", "cg_window_binding": "unverified",
                "action_allowed": false]
    }
}

// MARK: - Self-check

/// Pure-function checks of the ported code. Returns the names of the failed checks (empty when
/// all pass). It creates opaque AX references but reads no other process.
func accessibilitySelfCheck() -> [String] {
    var failed: [String] = []
    func check(_ name: String, _ ok: Bool) { if !ok { failed.append(name) } }
    check("kernel_start_rejects_non_positive", kernelStart(0) == nil && kernelStart(-1) == nil)
    let own = kernelStart(getpid())
    check("kernel_start_own_process", (own ?? 0) > 0 && kernelStart(getpid()) == own)
    let first = AXUIElementCreateApplication(101), second = AXUIElementCreateApplication(102)
    var candidates: [(element: AXUIElement, sources: [String])] = []
    addFallback(first, source: "main", to: &candidates)
    addFallback(first, source: "focused", to: &candidates)
    addFallback(second, source: "focused", to: &candidates)
    check("fallback_merges_equal_elements",
          candidates.count == 2 && candidates[0].sources == ["main", "focused"] && candidates[1].sources == ["focused"])
    let long = limited(String(repeating: "x", count: 300))
    let exact = limited(String(repeating: "x", count: maxString))
    check("limited_truncates", long.text.count == maxString && long.truncated
          && exact.text.count == maxString && !exact.truncated)
    var point = CGPoint(x: -17, y: 25)
    var size = CGSize(width: 12, height: 34)
    if let p = AXValueCreate(.cgPoint, &point), let s = AXValueCreate(.cgSize, &size) {
        check("ax_values", pointValue(p) == point && sizeValue(s) == size && pointValue(s) == nil)
    } else { check("ax_values", false) }
    // Creating an opaque AX reference does not contact or control the process.
    let fakeArray = [AXUIElementCreateApplication(42)] as CFArray
    check("elements_limits", elements(fakeArray)?.0.count == 1 && elements(fakeArray, limit: 0)?.0.count == 0
          && elements("not-an-array" as CFString) == nil)
    let budget = AXBudget()
    var textResult: [String: Any] = [:]
    markTextTruncated(&textResult, field: "title", truncated: long.truncated, budget: budget)
    markTextTruncated(&textResult, field: "role", truncated: exact.truncated, budget: budget)
    recordActions(["AXPress", String(repeating: "a", count: 300)], result: &textResult, budget: budget)
    check("truncation_markers", textResult["title_truncated"] as? Bool == true && textResult["role_truncated"] == nil
          && textResult["action_names_truncated_indices"] as? [Int] == [1] && budget.truncated)
    budget.count = maxNodes
    check("budget_exhausts", !budget.available() && budget.truncated)
    return failed
}
