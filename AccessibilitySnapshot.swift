// Read-only process-level AX snapshot. It does not activate or operate apps.
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

private enum SnapshotFailure: Error, CustomStringConvertible {
    case invalidArguments, accessibilityUnavailable, processUnavailable, processChanged
    case windowsUnavailable(Int32), invalidWindowsType, subtreeUnavailable
    var description: String {
        switch self {
        case .invalidArguments: return "expected a positive process ID"
        case .accessibilityUnavailable: return "accessibility permission is not granted"
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
        case .accessibilityUnavailable: return "accessibility_unavailable"
        case .processUnavailable: return "process_unavailable"
        case .processChanged: return "process_changed"
        case .windowsUnavailable: return "ax_windows_unavailable"
        case .invalidWindowsType: return "ax_windows_invalid_type"
        case .subtreeUnavailable: return "ax_subtree_unavailable"
        }
    }
}

private func parsePID(_ args: [String]) throws -> pid_t {
    guard args.count == 1, let pid = Int32(args[0]), pid > 0 else { throw SnapshotFailure.invalidArguments }
    return pid
}
private func limited(_ value: String) -> (text: String, truncated: Bool) {
    let prefix = String(value.prefix(maxString))
    return (prefix, value.count > maxString)
}
private func emit(_ value: [String: Any]) throws {
    let data = try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data([10]))
}
private func failure(_ error: SnapshotFailure) -> [String: Any] {
    ["schema_version": 1, "status": error.code, "reason": error.description,
     "windows": NSNull(), "action_allowed": false]
}

private func validBundleID(_ value: String) -> Bool {
    !value.isEmpty && value.utf8.count <= 255 && value.utf8.allSatisfy {
        (48...57).contains($0) || (65...90).contains($0) || (97...122).contains($0) || $0 == 45 || $0 == 46
    }
}

// Use the same kernel source in discovery, identity and AX snapshots. AppKit's
// launchDate is nil for some running apps, including Finder on this host.
private func kernelStart(_ pid: pid_t) -> Double? {
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

private func findBundle(_ bundle: String) throws -> [String: Any] {
    guard validBundleID(bundle) else { throw SnapshotFailure.invalidArguments }
    let apps = NSRunningApplication.runningApplications(withBundleIdentifier: bundle)
    guard apps.count <= 128 else { throw SnapshotFailure.processUnavailable }
    var rows: [[String: Any]] = []
    var incomplete = 0
    for app in apps {
        guard !app.isTerminated, app.processIdentifier > 0, app.bundleIdentifier == bundle,
              let launch = kernelStart(app.processIdentifier), let executable = app.executableURL?.path,
              executable.hasPrefix("/"), executable.utf8.count <= 4096 else {
            incomplete += 1; continue
        }
        let appPath = app.bundleURL?.path
        if let path = appPath, (!path.hasPrefix("/") || path.utf8.count > 4096 || path.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) })) {
            incomplete += 1; continue
        }
        rows.append(["pid":app.processIdentifier, "bundle_id":bundle,
                     "process_start_unix_seconds":launch, "process_start_source":"kernel_sysctl_kern_proc", "executable":executable,
                     "app_bundle_path":appPath as Any? ?? NSNull()])
    }
    rows.sort { ($0["pid"] as? Int32 ?? 0) < ($1["pid"] as? Int32 ?? 0) }
    return ["status":"ok", "action_allowed":false, "bundle_id":bundle,
            "incomplete_count":incomplete, "candidates":rows]
}

private func processIdentity(pid: pid_t) throws -> [String: Any] {
    guard let app = NSRunningApplication(processIdentifier: pid), let start = kernelStart(pid) else {
        throw SnapshotFailure.processUnavailable
    }
    return ["status": "ok", "pid": pid, "process_start_unix_seconds": start, "process_start_source":"kernel_sysctl_kern_proc",
            "bundle_id": app.bundleIdentifier as Any? ?? NSNull(),
            "executable": app.executableURL?.path as Any? ?? NSNull(), "action_allowed": false]
}

private final class Budget {
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

private func markTextTruncated(_ result: inout [String: Any], field: String, truncated: Bool, budget: Budget) {
    if truncated { result["\(field)_truncated"] = true; budget.truncated = true }
}

private func recordActions(_ names: [String], result: inout [String: Any], budget: Budget) {
    let limitedNames = names.prefix(maxActions).map(limited)
    result["actions"] = limitedNames.map(\.text)
    let truncatedIndices = limitedNames.enumerated().compactMap { $0.element.truncated ? $0.offset : nil }
    if !truncatedIndices.isEmpty { result["action_names_truncated_indices"] = truncatedIndices; budget.truncated = true }
    if names.count > maxActions { result["actions_truncated"] = true; budget.truncated = true }
}

private func attr(_ element: AXUIElement, _ name: CFString, _ budget: Budget) -> (CFTypeRef?, Int32?) {
    guard budget.available() else { return (nil, nil) }
    var result: CFTypeRef?
    let status = AXUIElementCopyAttributeValue(element, name, &result)
    return status == .success ? (result, nil) : (nil, status.rawValue)
}
private func stringAttr(_ element: AXUIElement, _ name: CFString, _ budget: Budget) -> (String?, Int32?, Bool) {
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
private func node(_ element: AXUIElement, path: [Int], depth: Int, ownerPID: pid_t, budget: Budget, windowsOnly: Bool = false) -> [String: Any]? {
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

private func subtree(pid: pid_t, source: String, indexes: [Int]) throws -> [String: Any] {
    guard AXIsProcessTrusted(), NSRunningApplication(processIdentifier: pid) != nil, let start = kernelStart(pid),
          indexes.count <= 8, indexes.allSatisfy({ $0 >= 0 && $0 < maxNodes }) else { throw SnapshotFailure.subtreeUnavailable }
    let root = AXUIElementCreateApplication(pid), budget = Budget()
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

private func snapshot(pid: pid_t, windowsOnly: Bool = false) throws -> [String: Any] {
    // This passive check never invokes an operating-system permission prompt.
    guard AXIsProcessTrusted() else { throw SnapshotFailure.accessibilityUnavailable }
    guard let app = NSRunningApplication(processIdentifier: pid), let start = kernelStart(pid) else { throw SnapshotFailure.processUnavailable }
    let root = AXUIElementCreateApplication(pid)
    _ = AXUIElementSetMessagingTimeout(root, 0.2)
    var raw: CFTypeRef?
    let status = AXUIElementCopyAttributeValue(root, kAXWindowsAttribute as CFString, &raw)
    guard status == .success else { throw SnapshotFailure.windowsUnavailable(status.rawValue) }
    guard let (windows, windowCount) = elements(raw, limit: maxWindows) else { throw SnapshotFailure.invalidWindowsType }
    let budget = Budget()
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

private func selfTest() throws {
    guard kernelStart(0) == nil, kernelStart(-1) == nil,
          let firstStart = kernelStart(getpid()), firstStart > 0,
          kernelStart(getpid()) == firstStart else { throw SnapshotFailure.processUnavailable }
    let first = AXUIElementCreateApplication(101), second = AXUIElementCreateApplication(102)
    var candidates: [(element: AXUIElement, sources: [String])] = []
    addFallback(first, source: "main", to: &candidates)
    addFallback(first, source: "focused", to: &candidates)
    addFallback(second, source: "focused", to: &candidates)
    guard candidates.count == 2, candidates[0].sources == ["main", "focused"], candidates[1].sources == ["focused"] else {
        throw SnapshotFailure.invalidArguments
    }
    guard validBundleID("com.example.app"), !validBundleID(""), !validBundleID("com.example/other"),
          !validBundleID(String(repeating: "a", count: 256)) else { throw SnapshotFailure.invalidArguments }
    guard try parsePID(["42"]) == 42 else { throw SnapshotFailure.invalidArguments }
    var rejected = false
    do { _ = try parsePID(["0"]) } catch SnapshotFailure.invalidArguments { rejected = true }
    let long = limited(String(repeating: "x", count: 300))
    let exact = limited(String(repeating: "x", count: maxString))
    guard rejected, long.text.count == maxString, long.truncated,
          exact.text.count == maxString, !exact.truncated else { throw SnapshotFailure.invalidArguments }
    var point = CGPoint(x: -17, y: 25)
    var size = CGSize(width: 12, height: 34)
    guard let p = AXValueCreate(.cgPoint, &point), let s = AXValueCreate(.cgSize, &size),
          pointValue(p) == point, sizeValue(s) == size, pointValue(s) == nil else {
        throw SnapshotFailure.invalidArguments
    }
    // Creating an opaque AX reference does not contact or control the process.
    let fake = AXUIElementCreateApplication(42)
    let fakeArray = [fake] as CFArray
    guard elements(fakeArray)?.0.count == 1, elements(fakeArray, limit: 0)?.0.count == 0,
          elements("not-an-array" as CFString) == nil else {
        throw SnapshotFailure.invalidArguments
    }
    let budget = Budget()
    var textResult: [String: Any] = [:]
    markTextTruncated(&textResult, field: "title", truncated: long.truncated, budget: budget)
    markTextTruncated(&textResult, field: "role", truncated: exact.truncated, budget: budget)
    recordActions(["AXPress", String(repeating: "a", count: 300)], result: &textResult, budget: budget)
    guard textResult["title_truncated"] as? Bool == true, textResult["role_truncated"] == nil,
          textResult["action_names_truncated_indices"] as? [Int] == [1], budget.truncated else {
        throw SnapshotFailure.invalidArguments
    }
    budget.count = maxNodes
    guard !budget.available(), budget.truncated else { throw SnapshotFailure.invalidArguments }
    let failed = failure(.processUnavailable)
    guard failed["windows"] is NSNull, failed["action_allowed"] as? Bool == false else { throw SnapshotFailure.invalidArguments }
    _ = try JSONSerialization.data(withJSONObject: failed)
    try emit(["status": "self_test_passed", "live_ui_access": false])
}

@main
struct Main {
    static func main() {
        let args = Array(CommandLine.arguments.dropFirst())
        if args == ["--self-test"] {
            do { try selfTest(); exit(0) } catch { fputs("self-test failed: \(error)\n", stderr); exit(1) }
        }
        if args.count == 2 && args[0] == "--find-bundle" {
            do { try emit(findBundle(args[1])); exit(0) }
            catch let error as SnapshotFailure {
                do { try emit(failure(error)) } catch { fputs("cannot encode failure\n", stderr); exit(1) }
                exit(2)
            } catch { fputs("unexpected bundle lookup failure: \(error)\n", stderr); exit(1) }
        }
        if args.count == 2 && args[0] == "--identity" {
            do { try emit(processIdentity(pid: parsePID([args[1]]))); exit(0) }
            catch let error as SnapshotFailure {
                do { try emit(failure(error)) } catch { fputs("cannot encode failure\n", stderr); exit(1) }
                exit(error.code == "invalid_arguments" ? 2 : 3)
            } catch { fputs("unexpected identity failure: \(error)\n", stderr); exit(1) }
        }
        if args.count == 4 && args[0] == "--subtree" {
            do {
                let pid = try parsePID([args[1]])
                let parts = args[3] == "-" ? [] : args[3].split(separator:",", omittingEmptySubsequences:false).map(String.init)
                let indexes = parts.compactMap(Int.init)
                guard parts.count <= 8, parts.allSatisfy({ !$0.isEmpty && $0.utf8.allSatisfy({ (48...57).contains($0) }) }),
                      indexes.count == parts.count else { throw SnapshotFailure.invalidArguments }
                try emit(subtree(pid:pid,source:args[2],indexes:indexes)); exit(0)
            } catch let error as SnapshotFailure {
                do { try emit(failure(error)) } catch { fputs("cannot encode failure\n", stderr); exit(1) }
                exit(error.code == "invalid_arguments" ? 2 : 3)
            } catch { fputs("unexpected AX subtree failure: \(error)\n", stderr); exit(1) }
        }
        let windowsOnly = args.count == 2 && args[0] == "--windows-only"
        let pidArgs = windowsOnly ? [args[1]] : args
        do { try emit(snapshot(pid: parsePID(pidArgs), windowsOnly: windowsOnly)); exit(0) }
        catch let error as SnapshotFailure {
            do { try emit(failure(error)) } catch { fputs("cannot encode failure\n", stderr); exit(1) }
            exit(error.code == "invalid_arguments" ? 2 : 3)
        } catch { fputs("unexpected AX failure: \(error)\n", stderr); exit(1) }
    }
}
