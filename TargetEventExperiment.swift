// Opt-in experiment. It never accepts an external PID or window ID.
import AppKit
import ApplicationServices
import Foundation
import Darwin

private enum ProbeError: Error, CustomStringConvertible {
    case reason(String)
    var description: String { if case .reason(let value) = self { return value }; return "unknown" }
}

private struct ProcessIdentity: Equatable {
    let pid: pid_t
    let parent: pid_t
    let startSeconds: Int64
    let startMicroseconds: Int32
    let executable: String
}

// Kernel data and executable path are checked independently of child IPC.
private func processIdentity(_ pid: pid_t) throws -> ProcessIdentity {
    guard pid > 0 else { throw ProbeError.reason("invalid process") }
    var mib = [CTL_KERN, KERN_PROC, KERN_PROC_PID, pid]
    var info = kinfo_proc()
    var size = MemoryLayout<kinfo_proc>.size
    guard sysctl(&mib, 4, &info, &size, nil, 0) == 0,
          size == MemoryLayout<kinfo_proc>.size, info.kp_proc.p_pid == pid else {
        throw ProbeError.reason("process identity unavailable")
    }
    var path = [CChar](repeating: 0, count: 4 * Int(MAXPATHLEN))
    guard proc_pidpath(pid, &path, UInt32(path.count)) > 0 else {
        throw ProbeError.reason("process executable unavailable")
    }
    let exe = URL(fileURLWithPath: String(cString: path)).resolvingSymlinksInPath().path
    let start = info.kp_proc.p_starttime
    guard start.tv_sec > 0 else { throw ProbeError.reason("process start unavailable") }
    return ProcessIdentity(pid: pid, parent: info.kp_eproc.e_ppid,
        startSeconds: Int64(start.tv_sec), startMicroseconds: Int32(start.tv_usec), executable: exe)
}

private struct WindowEntry: Equatable { let id: UInt32; let owner: pid_t }

private struct Sample: Equatable {
    let started: Double
    let finished: Double
    let front: pid_t
    let focused: pid_t
    let focusedHash: CFHashCode
    let x: Double
    let y: Double
    let mouseButtonsDown: Bool
    let modifierFlags: UInt64
    let windowOrder: [WindowEntry]
    func sameState(_ other: Sample) -> Bool {
        front == other.front && focused == other.focused && focusedHash == other.focusedHash &&
        x == other.x && y == other.y && mouseButtonsDown == other.mouseButtonsDown && modifierFlags == other.modifierFlags &&
        windowOrder == other.windowOrder
    }
}

// Private samples stay in memory. The result never stores user cursor positions.
private func sample() throws -> Sample {
    let started = ProcessInfo.processInfo.systemUptime
    guard let app = NSWorkspace.shared.frontmostApplication, let event = CGEvent(source: nil) else {
        throw ProbeError.reason("cannot sample frontmost app or pointer")
    }
    guard let windows = CGWindowListCopyWindowInfo(.optionOnScreenOnly, kCGNullWindowID) as? [[String: Any]] else {
        throw ProbeError.reason("cannot sample window order")
    }
    let order: [WindowEntry] = windows.compactMap {
        guard let id = ($0[kCGWindowNumber as String] as? NSNumber)?.uint32Value,
              let owner = ($0[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value else { return nil }
        return WindowEntry(id: id, owner: owner)
    }
    guard !order.isEmpty, order.count == windows.count, order.count <= 4096,
          !order.contains(where: { $0.id == 0 || $0.owner <= 0 }), Set(order.map(\.id)).count == order.count else {
        throw ProbeError.reason("cannot sample window order")
    }
    let system = AXUIElementCreateSystemWide()
    _ = AXUIElementSetMessagingTimeout(system, 0.2)
    func focusedApplication() throws -> (element: AXUIElement, pid: pid_t) {
        var value: CFTypeRef?
        let status = AXUIElementCopyAttributeValue(system, kAXFocusedApplicationAttribute as CFString, &value)
        guard status == .success else {
            switch status {
            case .failure: throw ProbeError.reason("focused_app_ax_failure")
            case .cannotComplete: throw ProbeError.reason("focused_app_ax_cannot_complete")
            case .noValue: throw ProbeError.reason("focused_app_ax_no_value")
            case .invalidUIElement: throw ProbeError.reason("focused_app_ax_invalid_element")
            default: throw ProbeError.reason("focused_app_ax_other")
            }
        }
        guard let value, CFGetTypeID(value) == AXUIElementGetTypeID() else {
            throw ProbeError.reason("focused_app_missing_or_wrong_type")
        }
        let element = unsafeBitCast(value, to: AXUIElement.self)
        var pid: pid_t = 0
        guard AXUIElementGetPid(element, &pid) == .success, pid > 0 else {
            throw ProbeError.reason("cannot sample focused application")
        }
        return (element, pid)
    }
    let focusBefore = try focusedApplication()
    var raw: CFTypeRef?
    guard AXUIElementCopyAttributeValue(system, kAXFocusedUIElementAttribute as CFString, &raw) == .success,
          let value = raw, CFGetTypeID(value) == AXUIElementGetTypeID() else {
        throw ProbeError.reason("cannot sample focused element")
    }
    let element = unsafeBitCast(value, to: AXUIElement.self)
    var pid: pid_t = 0
    guard AXUIElementGetPid(element, &pid) == .success, pid > 0,
          event.location.x.isFinite, event.location.y.isFinite else { throw ProbeError.reason("invalid sample") }
    let focusAfter = try focusedApplication()
    guard focusSourcesAgree(front: app.processIdentifier, before: focusBefore.pid, after: focusAfter.pid,
                            element: pid, sameApplication: CFEqual(focusBefore.element, focusAfter.element)) else {
        if focusBefore.pid != app.processIdentifier { throw ProbeError.reason("focus_before_not_front") }
        if focusAfter.pid != app.processIdentifier { throw ProbeError.reason("focus_after_not_front") }
        if pid != app.processIdentifier { throw ProbeError.reason("focus_element_not_front") }
        throw ProbeError.reason("focus_application_changed")
    }
    let buttons = [CGMouseButton.left, .right, .center].contains { CGEventSource.buttonState(.hidSystemState, button: $0) }
    let flags = CGEventSource.flagsState(.hidSystemState).rawValue
    return Sample(started: started, finished: ProcessInfo.processInfo.systemUptime,
                  front: app.processIdentifier, focused: pid, focusedHash: CFHash(element),
                  x: event.location.x, y: event.location.y, mouseButtonsDown: buttons, modifierFlags: flags, windowOrder: order)
}

private func focusSourcesAgree(front: pid_t, before: pid_t, after: pid_t, element: pid_t, sameApplication: Bool) -> Bool {
    front > 0 && before == front && after == front && element == front && sameApplication
}

private struct PartialSample {
    let front: pid_t
    let x: Double
    let y: Double
    let order: [WindowEntry]
    let frontFocusHash: CFHashCode?
    let frontFocusElement: AXUIElement?
    let frontWindowHash: CFHashCode?
}

// Capture only the independent signals when AX focus is unavailable. Nothing
// from this structure is serialized; results contain comparisons only.
private func partialSample() -> PartialSample? {
    guard let app = NSWorkspace.shared.frontmostApplication, let event = CGEvent(source: nil),
          event.location.x.isFinite, event.location.y.isFinite,
          let windows = CGWindowListCopyWindowInfo(.optionOnScreenOnly, kCGNullWindowID) as? [[String: Any]] else { return nil }
    let order: [WindowEntry] = windows.compactMap {
        guard let id = ($0[kCGWindowNumber as String] as? NSNumber)?.uint32Value,
              let owner = ($0[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value, id > 0, owner > 0 else { return nil }
        return WindowEntry(id: id, owner: owner)
    }
    guard !order.isEmpty, order.count == windows.count, order.count <= 4096,
          Set(order.map(\.id)).count == order.count else { return nil }
    let frontElement = AXUIElementCreateApplication(app.processIdentifier)
    _ = AXUIElementSetMessagingTimeout(frontElement, 0.1)
    var raw: CFTypeRef?
    let status = AXUIElementCopyAttributeValue(frontElement, kAXFocusedUIElementAttribute as CFString, &raw)
    var focusHash: CFHashCode?
    var focusElement: AXUIElement?
    if status == .success, let value = raw, CFGetTypeID(value) == AXUIElementGetTypeID() {
        let element = unsafeBitCast(value, to: AXUIElement.self)
        var pid: pid_t = 0
        if AXUIElementGetPid(element, &pid) == .success, pid == app.processIdentifier {
            focusHash = CFHash(element); focusElement = element
        }
    }
    var windowRaw: CFTypeRef?
    let windowStatus = AXUIElementCopyAttributeValue(frontElement, kAXFocusedWindowAttribute as CFString, &windowRaw)
    var windowHash: CFHashCode?
    if windowStatus == .success, let value = windowRaw, CFGetTypeID(value) == AXUIElementGetTypeID() {
        let window = unsafeBitCast(value, to: AXUIElement.self)
        var pid: pid_t = 0
        if AXUIElementGetPid(window, &pid) == .success, pid == app.processIdentifier { windowHash = CFHash(window) }
    }
    return PartialSample(front: app.processIdentifier, x: event.location.x, y: event.location.y,
                         order: order, frontFocusHash: focusHash, frontFocusElement: focusElement, frontWindowHash: windowHash)
}

// Read the retained baseline element only. Do not read its text or other content.
private func focusedFlag(_ element: AXUIElement?) -> Bool? {
    guard let element else { return nil }
    _ = AXUIElementSetMessagingTimeout(element, 0.1)
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, kAXFocusedAttribute as CFString, &value) == .success,
          let value, CFGetTypeID(value) == CFBooleanGetTypeID() else { return nil }
    return CFBooleanGetValue(unsafeBitCast(value, to: CFBoolean.self))
}

private func compareObservedHashes(_ baseline: CFHashCode?, _ values: [CFHashCode?]) -> Any {
    let available = values.compactMap { $0 }
    guard let baseline, !available.isEmpty else { return NSNull() }
    return available.allSatisfy { $0 == baseline }
}

private final class Monitor {
    private let lock = NSLock()
    private var values: [Sample] = []
    private var failures = 0
    private var failureKinds: [String: Int] = [:]
    private var phaseFailures: [String: Int] = [:]
    private var partials: [PartialSample] = []
    private var partialErrors = 0
    private var baselineWindowHash: CFHashCode?
    private var baselineElement: AXUIElement?
    private var baselineElementFocused: Bool?
    private var baselineElementFocusSamples: [Bool?] = []
    private var phase = "baseline"
    private let timer = DispatchSource.makeTimerSource(queue: DispatchQueue(label: "headless-event-monitor"))
    func start() {
        timer.schedule(deadline: .now(), repeating: .milliseconds(20))
        timer.setEventHandler { [weak self] in
            guard let self else { return }
            do { let value = try sample(); self.lock.lock(); self.values.append(value); self.lock.unlock() }
            catch {
                let kind = sampleFailureKind(error)
                let partial = partialSample()
                self.lock.lock(); let baselineElement = self.baselineElement; self.lock.unlock()
                let retainedElementFocused = focusedFlag(baselineElement)
                self.lock.lock()
                self.baselineElementFocusSamples.append(retainedElementFocused)
                if let partial { self.partials.append(partial) } else { self.partialErrors += 1 }
                self.failures += 1; self.failureKinds[kind, default: 0] += 1
                self.phaseFailures["\(self.phase).\(kind)", default: 0] += 1; self.lock.unlock()
            }
        }
        timer.resume()
    }
    func snapshot() -> ([Sample], Int) { lock.lock(); defer { lock.unlock() }; return (values, failures) }
    func reset() { lock.lock(); values.removeAll(); failures = 0; failureKinds.removeAll(); phaseFailures.removeAll(); partials.removeAll(); partialErrors = 0; baselineWindowHash = nil; baselineElement = nil; baselineElementFocused = nil; baselineElementFocusSamples.removeAll(); lock.unlock() }
    func setBaselineWindow(_ sample: PartialSample?) {
        let flag = focusedFlag(sample?.frontFocusElement)
        lock.lock(); defer { lock.unlock() }
        guard let first = values.first, let sample,
              sample.front == first.front, sample.x == first.x, sample.y == first.y,
              sample.frontFocusHash == first.focusedHash else { return }
        baselineWindowHash = sample.frontWindowHash
        baselineElement = sample.frontFocusElement
        baselineElementFocused = flag
    }
    func setPhase(_ value: String) { lock.lock(); phase = value; lock.unlock() }
    func phaseSnapshot() -> [String: Int] { lock.lock(); defer { lock.unlock() }; return phaseFailures }
    func partialDiagnostics(excluding fixture: pid_t?) -> [String: Any] {
        lock.lock(); defer { lock.unlock() }
        guard let first = values.first, !partials.isEmpty else { return ["available": false, "count": partials.count] }
        let expectedOrder = first.windowOrder.filter { fixture == nil || $0.owner != fixture }
        return ["available": partialErrors == 0, "count": partials.count, "partial_errors": partialErrors,
            "front_matches_baseline": partials.allSatisfy { $0.front == first.front },
            "fixture_was_front": partials.contains { fixture != nil && $0.front == fixture },
            "cursor_matches_baseline": partials.allSatisfy { $0.x == first.x && $0.y == first.y },
            "baseline_element_focused": baselineElementFocused.map { $0 as Any } ?? NSNull(),
            "baseline_element_focus_true_count": baselineElementFocusSamples.filter { $0 == true }.count,
            "baseline_element_focus_false_count": baselineElementFocusSamples.filter { $0 == false }.count,
            "baseline_element_focus_unavailable_count": baselineElementFocusSamples.filter { $0 == nil }.count,
            "front_focused_window_baseline_available": baselineWindowHash != nil,
            "front_focused_window_available_count": partials.filter { $0.frontWindowHash != nil }.count,
            "front_focused_window_matches_baseline_when_available": compareObservedHashes(baselineWindowHash, partials.map(\.frontWindowHash)),
            "front_focused_element_available_count": partials.filter { $0.frontFocusHash != nil }.count,
            "front_focused_element_matches_baseline_when_available": compareObservedHashes(first.focusedHash, partials.map(\.frontFocusHash)),
            "window_order_matches_baseline": partials.allSatisfy {
                $0.order.filter { fixture == nil || $0.owner != fixture } == expectedOrder
            }]
    }
    func detailedSnapshot() -> ([Sample], Int, [String: Int]) {
        lock.lock(); defer { lock.unlock() }; return (values, failures, failureKinds)
    }
    func stop() { timer.cancel() }
}

// Only fixed categories leave the process; never include window names or positions.
private func sampleFailureKind(_ error: Error) -> String {
    switch error {
    case ProbeError.reason("cannot sample focused element"): return "focus_unavailable"
    case ProbeError.reason("cannot sample focused application"): return "focused_application_unavailable"
    case ProbeError.reason("focused_app_ax_failure"): return "focused_app_ax_failure"
    case ProbeError.reason("focused_app_ax_cannot_complete"): return "focused_app_ax_cannot_complete"
    case ProbeError.reason("focused_app_ax_no_value"): return "focused_app_ax_no_value"
    case ProbeError.reason("focused_app_ax_invalid_element"): return "focused_app_ax_invalid_element"
    case ProbeError.reason("focused_app_ax_other"): return "focused_app_ax_other"
    case ProbeError.reason("focused_app_missing_or_wrong_type"): return "focused_app_missing_or_wrong_type"
    case ProbeError.reason("focus_before_not_front"): return "focus_before_not_front"
    case ProbeError.reason("focus_after_not_front"): return "focus_after_not_front"
    case ProbeError.reason("focus_element_not_front"): return "focus_element_not_front"
    case ProbeError.reason("focus_application_changed"): return "focus_application_changed"
    case ProbeError.reason("cannot sample frontmost app or pointer"): return "foreground_or_pointer_unavailable"
    case ProbeError.reason("cannot sample window order"): return "window_order_unavailable"
    case ProbeError.reason("invalid sample"): return "invalid_sample"
    default: return "other"
    }
}

private func samplesUnchanged(_ values: [Sample], _ failures: Int, excluding fixture: pid_t? = nil) -> Bool {
    guard let first = values.first, values.count >= 3, failures == 0 else { return false }
    var previousFinished: Double?
    for value in values {
        guard value.started.isFinite, value.finished.isFinite, value.started >= 0,
              value.finished >= value.started, value.finished - value.started <= 0.25,
              !value.mouseButtonsDown,
              value.modifierFlags & ~CGEventFlags.maskNonCoalesced.rawValue == 0,
              value.front == first.front, value.focused == first.focused, value.focusedHash == first.focusedHash,
              value.x == first.x, value.y == first.y, value.modifierFlags == first.modifierFlags,
              value.windowOrder.filter({ fixture == nil || $0.owner != fixture }) == first.windowOrder.filter({ fixture == nil || $0.owner != fixture }) else { return false }
        if let previousFinished {
            guard value.started >= previousFinished, value.started - previousFinished <= 0.25 else { return false }
        }
        previousFinished = value.finished
    }
    return true
}

// Aggregates only; no app identities, key values, cursor coordinates or window IDs leave the process.
private func sampleDiagnostics(_ values: [Sample], _ failures: Int, excluding fixture: pid_t? = nil) -> [String: Any] {
    guard failures == 0, values.count >= 3, let first = values.first else {
        return ["available": false]
    }
    return ["available": true,
        "frontmost_unchanged": values.allSatisfy { $0.front == first.front },
        "focused_element_unchanged": values.allSatisfy { $0.focused == first.focused && $0.focusedHash == first.focusedHash },
        "cursor_unchanged": values.allSatisfy { $0.x == first.x && $0.y == first.y },
        "window_order_unchanged": values.allSatisfy { $0.windowOrder.filter { fixture == nil || $0.owner != fixture } == first.windowOrder.filter { fixture == nil || $0.owner != fixture } },
        "mouse_buttons_clear": values.allSatisfy { !$0.mouseButtonsDown },
        "modifier_flags_clear": values.allSatisfy { $0.modifierFlags & ~CGEventFlags.maskNonCoalesced.rawValue == 0 },
        "noncoalesced_seen": values.contains { $0.modifierFlags & CGEventFlags.maskNonCoalesced.rawValue != 0 },
        "caps_lock_seen": values.contains { $0.modifierFlags & CGEventFlags.maskAlphaShift.rawValue != 0 },
        "shift_seen": values.contains { $0.modifierFlags & CGEventFlags.maskShift.rawValue != 0 },
        "control_seen": values.contains { $0.modifierFlags & CGEventFlags.maskControl.rawValue != 0 },
        "option_seen": values.contains { $0.modifierFlags & CGEventFlags.maskAlternate.rawValue != 0 },
        "command_seen": values.contains { $0.modifierFlags & CGEventFlags.maskCommand.rawValue != 0 },
        "fn_seen": values.contains { $0.modifierFlags & CGEventFlags.maskSecondaryFn.rawValue != 0 }]
}

// Samples only bound what was observed. They cannot prove continuous isolation.
private func coversPost(_ values: [Sample], started: Double?, finished: Double?) -> Bool {
    guard let started, let finished, started.isFinite, finished.isFinite,
          started >= 0, finished >= started, finished - started <= 0.25,
          let before = values.last(where: { $0.finished <= started }) else { return false }
    let after = values.filter { $0.started >= finished }
    guard before.finished <= started, started - before.finished <= 0.25,
          let firstAfter = after.first, after.count >= 3,
          firstAfter.started - finished <= 0.25 else { return false }
    return true
}

private func isFresh(_ value: Sample, at time: Double) -> Bool {
    time.isFinite && value.finished.isFinite && time >= value.finished && time - value.finished <= 0.25
}


private struct WindowCandidate {
    let globalID: Int
    let pid: pid_t
    let title: String
    let frame: CGRect
}

// The child reports an AppKit number and geometry. Neither is a CG window ID.
// Fail closed unless the independent CG inventory has exactly one matching
// child/title entry and its geometry agrees with the child's reported frame.
private func selectFixtureWindow(_ candidates: [WindowCandidate], pid: pid_t,
                                 title: String, reportedFrame: CGRect) throws -> WindowCandidate {
    let matches = candidates.filter { $0.pid == pid && $0.title == title }
    guard pid > 0, !title.isEmpty, matches.count == 1, let candidate = matches.first,
          candidate.globalID > 0 else { throw ProbeError.reason("fixture window is not unique") }
    let rect = candidate.frame
    let actual = [rect.origin.x, rect.origin.y, rect.width, rect.height]
    let reported = [reportedFrame.origin.x, reportedFrame.origin.y, reportedFrame.width, reportedFrame.height]
    guard actual.allSatisfy({ $0.isFinite }), reported.allSatisfy({ $0.isFinite }),
          rect.width >= 100, rect.width <= 400, rect.height >= 70, rect.height <= 250,
          zip(actual, reported).allSatisfy({ abs($0 - $1) <= 0.5 }) else {
        let originMismatch = abs(rect.minX - reportedFrame.minX) > 0.5 || abs(rect.minY - reportedFrame.minY) > 0.5
        let sizeMismatch = abs(rect.width - reportedFrame.width) > 0.5 || abs(rect.height - reportedFrame.height) > 0.5
        if originMismatch && sizeMismatch {
            throw ProbeError.reason(String(format: "fixture geometry delta dx=%.1f dy=%.1f dw=%.1f dh=%.1f",
                rect.minX - reportedFrame.minX, rect.minY - reportedFrame.minY,
                rect.width - reportedFrame.width, rect.height - reportedFrame.height))
        }
        if originMismatch { throw ProbeError.reason("fixture origin mismatch") }
        if sizeMismatch { throw ProbeError.reason("fixture size mismatch") }
        throw ProbeError.reason("fixture geometry invalid")
    }
    return candidate
}

private func verifiedWindow(pid: pid_t, title: String, reportedFrame: CGRect) throws -> WindowCandidate {
    guard let entries = CGWindowListCopyWindowInfo(.optionAll, kCGNullWindowID) as? [[String: Any]] else {
        throw ProbeError.reason("window server unavailable")
    }
    let matchingEntries = entries.filter {
        ($0[kCGWindowOwnerPID as String] as? Int) == Int(pid) &&
        ($0[kCGWindowName as String] as? String) == title
    }
    guard matchingEntries.count == 1, let entry = matchingEntries.first,
          let id = entry[kCGWindowNumber as String] as? Int,
          let dict = entry[kCGWindowBounds as String] as? NSDictionary,
          let rect = CGRect(dictionaryRepresentation: dict) else {
        throw ProbeError.reason("fixture window identity or bounds unavailable")
    }
    return try selectFixtureWindow([WindowCandidate(globalID: id, pid: pid, title: title, frame: rect)],
                                   pid: pid, title: title, reportedFrame: reportedFrame)
}

private func eventForFixture(type: NSEvent.EventType, point: NSPoint, appkitWindow: Int,
                             pressure: Float) -> CGEvent? {
    guard let event = NSEvent.mouseEvent(with: type, location: point, modifierFlags: [],
        timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: appkitWindow,
        context: nil, eventNumber: 1, clickCount: 1, pressure: pressure),
        event.windowNumber == appkitWindow, let cg = event.cgEvent,
        NSEvent(cgEvent: cg)?.windowNumber == appkitWindow else { return nil }
    return cg
}

private func record(_ url: URL, kind: String, pid: pid_t, window: Int) -> Bool {
    guard let data = try? Data(contentsOf: url),
          let value = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
          value["kind"] as? String == kind, value["count"] as? Int == 1,
          value["pid"] as? Int == Int(pid), value["window_number"] as? Int == window,
          let x = value["x"] as? Double, let y = value["y"] as? Double,
          x.isFinite, y.isFinite, x >= 0, x < 240, y >= 0, y < 110 else { return false }
    return true
}

private func selfTest() throws {
    guard compareObservedHashes(nil, [1]) is NSNull,
          compareObservedHashes(1, [nil]) is NSNull,
          compareObservedHashes(1, [nil, 1]) as? Bool == true,
          compareObservedHashes(1, [1, 2]) as? Bool == false else {
        throw ProbeError.reason("focus diagnostic comparison failed")
    }
    func sample(_ time: Double, _ x: Double = 4, _ buttons: Bool = false, _ order: [WindowEntry] = [WindowEntry(id: 1, owner: 2)], flags: UInt64 = 0) -> Sample {
        Sample(started: time, finished: time + 0.01, front: 2, focused: 2, focusedHash: 3,
               x: x, y: 5, mouseButtonsDown: buttons, modifierFlags: flags, windowOrder: order)
    }
    let a = sample(1), b = sample(1.05), c = sample(1.10)
    let fixture = sample(1.10, 4, false, [WindowEntry(id: 1, owner: 2), WindowEntry(id: 99, owner: 42)])
    guard samplesUnchanged([a, b, c], 0), !samplesUnchanged([a, b], 0),
          !samplesUnchanged([a, b, sample(1.10, 5)], 0), !samplesUnchanged([a, b, c], 1),
          !samplesUnchanged([a, b, sample(1.10, 4, true)], 0),
          !samplesUnchanged([a, b, fixture], 0), samplesUnchanged([a, b, fixture], 0, excluding: 42),
          !coversPost([a, b, c], started: 1.06, finished: 1.07),
          samplesUnchanged([sample(1, flags: CGEventFlags.maskNonCoalesced.rawValue), sample(1.05, flags: CGEventFlags.maskNonCoalesced.rawValue), sample(1.1, flags: CGEventFlags.maskNonCoalesced.rawValue)], 0),
          !samplesUnchanged([a, b, sample(1.1, flags: CGEventFlags.maskNonCoalesced.rawValue)], 0),
          !samplesUnchanged([sample(1, flags: CGEventFlags.maskShift.rawValue), sample(1.05, flags: CGEventFlags.maskShift.rawValue), sample(1.1, flags: CGEventFlags.maskShift.rawValue)], 0) else {
        throw ProbeError.reason("sample comparison failed")
    }
    let frame = CGRect(x: 10, y: 20, width: 240, height: 130)
    let candidate = WindowCandidate(globalID: 88, pid: 42, title: "fixture nonce", frame: frame)
    let decoy = WindowCandidate(globalID: 77, pid: 43, title: "fixture nonce", frame: frame)
    guard try selectFixtureWindow([candidate, decoy], pid: 42, title: "fixture nonce", reportedFrame: frame).globalID == 88,
          (try? selectFixtureWindow([candidate, candidate], pid: 42, title: "fixture nonce", reportedFrame: frame)) == nil,
          (try? selectFixtureWindow([candidate], pid: 42, title: "fixture nonce", reportedFrame: frame.offsetBy(dx: 2, dy: 0))) == nil,
          (try? selectFixtureWindow([candidate], pid: 42, title: "wrong nonce", reportedFrame: frame)) == nil else {
        throw ProbeError.reason("window identity self-test failed")
    }
    // Synthetic number, no window creation or event post. This only checks
    // in-process NSEvent -> CGEvent -> NSEvent round-trip on this runtime.
    guard eventForFixture(type: .leftMouseDown, point: NSPoint(x: 100, y: 120), appkitWindow: 424242, pressure: 1) != nil,
          eventForFixture(type: .leftMouseUp, point: NSPoint(x: 100, y: 120), appkitWindow: 424242, pressure: 0) != nil else {
        throw ProbeError.reason("event window-number round-trip failed")
    }
}

private func runFixture(observeOnly: Bool, hidden: Bool = false, ignoresMouse: Bool = false) throws -> [String: Any] {
    guard (!hidden && !ignoresMouse) || observeOnly else { throw ProbeError.reason("diagnostic modes are observation only") }
    guard AXIsProcessTrusted(), CGPreflightPostEventAccess() else { throw ProbeError.reason("required permissions unavailable; no prompt requested") }
    let base = URL(fileURLWithPath: CommandLine.arguments[0]).standardizedFileURL.deletingLastPathComponent()
    let fixture = base.appendingPathComponent("TargetEventFixture.app/Contents/MacOS/TargetEventFixture")
        .resolvingSymlinksInPath()
    guard FileManager.default.isExecutableFile(atPath: fixture.path) else { throw ProbeError.reason("fixture executable missing") }
    let nonce = UUID().uuidString
    let dir = URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true).appendingPathComponent("target-event-\(nonce)")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
    let ready = dir.appendingPathComponent("ready.json")
    let downPath = dir.appendingPathComponent("down-1.json")
    let upPath = dir.appendingPathComponent("up-1.json")
    let monitor = Monitor()
    monitor.start()
    defer { monitor.stop() }
    var result: [String: Any] = ["status": "stopped", "action_attempted": false,
        "general_input_authorized": false,
        "note": "Sampling can miss brief changes. Targeted events may affect focus or cursor; a fixture cannot prove another app is safe."]
    var launched: Process?
    var postStarted: Double?
    var postFinished: Double?
    do {
        // Wait for an uninterrupted quiet interval. A moving user is never
        // asked to stop, and every later sample remains part of the audit.
        let baselineDeadline = ProcessInfo.processInfo.systemUptime + 10
        var (samples, errors) = monitor.snapshot()
        var stable = false
        while ProcessInfo.processInfo.systemUptime < baselineDeadline {
            (samples, errors) = monitor.snapshot()
            if errors > 0 || (samples.count >= 3 && !samplesUnchanged(samples, errors)) {
                monitor.reset()
            } else if samplesUnchanged(samples, errors), let first = samples.first, let last = samples.last,
                      last.finished - first.started >= 0.2, isFresh(last, at: ProcessInfo.processInfo.systemUptime) {
                stable = true; break
            }
            Thread.sleep(forTimeInterval: 0.04)
        }
        guard stable else { throw ProbeError.reason("cannot establish stable baseline") }
        // Secondary focus information is diagnostic only; it cannot relax the
        // mandatory system-wide focus checks or authorize posting.
        monitor.setBaselineWindow(partialSample())
        (samples, errors) = monitor.snapshot()
        guard samplesUnchanged(samples, errors), let latest = samples.last,
              isFresh(latest, at: ProcessInfo.processInfo.systemUptime) else {
            throw ProbeError.reason("baseline changed during diagnostic")
        }
        let child = Process()
        child.executableURL = fixture
        child.arguments = ["--child-fixture", String(getpid()), nonce]
        var env = ProcessInfo.processInfo.environment
        env["TARGET_EVENT_FIXTURE_DIR"] = dir.path
        if hidden { env["TARGET_EVENT_FIXTURE_HIDDEN"] = "1" }
        else { env.removeValue(forKey: "TARGET_EVENT_FIXTURE_HIDDEN") }
        if ignoresMouse { env["TARGET_EVENT_FIXTURE_IGNORE_MOUSE"] = "1" }
        else { env.removeValue(forKey: "TARGET_EVENT_FIXTURE_IGNORE_MOUSE") }
        child.environment = env
        child.standardOutput = FileHandle.nullDevice
        let stderrURL = dir.appendingPathComponent("fixture-stderr.txt")
        let fd = open(stderrURL.path, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW, mode_t(0o600))
        guard fd >= 0 else { throw ProbeError.reason("fixture diagnostics unavailable") }
        let stderrHandle = FileHandle(fileDescriptor: fd, closeOnDealloc: true)
        child.standardError = stderrHandle
        monitor.setPhase("launching")
        do { try child.run() } catch { try? stderrHandle.close(); throw error }
        try? stderrHandle.close()
        launched = child
        let pid = child.processIdentifier
        result["fixture_pid"] = pid
        let deadline = ProcessInfo.processInfo.systemUptime + 6
        while child.isRunning && !FileManager.default.fileExists(atPath: ready.path) && ProcessInfo.processInfo.systemUptime < deadline {
            Thread.sleep(forTimeInterval: 0.05)
        }
        let observedApp = NSRunningApplication(processIdentifier: pid)
        result["fixture_start_diagnostics"] = ["process_running": child.isRunning,
            "running_app_available": observedApp != nil, "launch_date_available": observedApp?.launchDate != nil,
            "executable_matches": observedApp?.executableURL?.resolvingSymlinksInPath().path == fixture.path,
            "bundle_matches": observedApp?.bundleIdentifier == "org.local.rust-computer-use.target-event-fixture",
            "ready_file_present": FileManager.default.fileExists(atPath: ready.path)]
        let identityProcess = try processIdentity(pid)
        guard child.isRunning, identityProcess.parent == getpid(), identityProcess.executable == fixture.path,
              let app = observedApp,
              app.executableURL?.resolvingSymlinksInPath().path == fixture.path,
              app.bundleIdentifier == "org.local.rust-computer-use.target-event-fixture",
              let data = try? Data(contentsOf: ready),
              let identity = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              identity["pid"] as? Int == Int(pid), identity["nonce"] as? String == nonce,
              let window = identity["appkit_window_number"] as? Int, window > 0,
              let reported = identity["quartz_frame"] as? [String: Double],
              let x = reported["x"], let y = reported["y"],
              let width = reported["width"], let height = reported["height"] else {
            throw ProbeError.reason("fixture process identity not verified")
        }
        if hidden {
            guard identity["visible"] as? Bool == false else { throw ProbeError.reason("hidden fixture not verified") }
            monitor.setPhase("hidden_ready")
            Thread.sleep(forTimeInterval: 0.5)
            guard child.isRunning, (try? processIdentity(pid)) == identityProcess else {
                throw ProbeError.reason("hidden fixture changed during observation")
            }
            result["observation_ready"] = true
            result["fixture_hidden"] = true
            throw ProbeError.reason("observation complete")
        }
        if ignoresMouse {
            guard identity["ignores_mouse"] as? Bool == true else { throw ProbeError.reason("mouse ignoring not verified") }
            result["fixture_ignores_mouse"] = true
        }
        let title = "Target Event Fixture \(nonce)"
        let reportedFrame = CGRect(x: x, y: y, width: width, height: height)
        var cgWindow: WindowCandidate?
        var lastWindowError: Error = ProbeError.reason("fixture window unavailable")
        let windowDeadline = min(deadline, ProcessInfo.processInfo.systemUptime + 1.5)
        repeat {
            guard child.isRunning, (try? processIdentity(pid)) == identityProcess else {
                throw ProbeError.reason("fixture changed while awaiting window")
            }
            do { cgWindow = try verifiedWindow(pid: pid, title: title, reportedFrame: reportedFrame); break }
            catch { lastWindowError = error }
            Thread.sleep(forTimeInterval: 0.05)
        } while ProcessInfo.processInfo.systemUptime < windowDeadline
        guard let cgWindow else { throw lastWindowError }
        monitor.setPhase("window_ready")
        let frame = cgWindow.frame
        if observeOnly {
            // Observation never constructs or posts an input event. Keep the
            // fixture visible long enough for focus sampling before teardown.
            Thread.sleep(forTimeInterval: 0.5)
            guard child.isRunning, (try? processIdentity(pid)) == identityProcess else {
                throw ProbeError.reason("fixture changed during observation")
            }
            result["observation_ready"] = true
            throw ProbeError.reason("observation complete")
        }
        let quartzPoint = CGPoint(x: frame.midX, y: frame.midY)
        // AppKit cannot resolve another process's window number. Its constructor
        // expects a screen point with a bottom-left origin for this conversion.
        let point = NSPoint(x: quartzPoint.x, y: CGDisplayBounds(CGMainDisplayID()).height - quartzPoint.y)
        guard let down = eventForFixture(type: .leftMouseDown, point: point, appkitWindow: window, pressure: 1),
              let up = eventForFixture(type: .leftMouseUp, point: point, appkitWindow: window, pressure: 0) else {
            throw ProbeError.reason("cannot create events with the fixture window number")
        }
        Thread.sleep(forTimeInterval: 0.15)
        (samples, errors) = monitor.snapshot()
        guard samplesUnchanged(samples, errors, excluding: launched?.processIdentifier), child.isRunning,
              let check = NSRunningApplication(processIdentifier: pid), (try? processIdentity(pid)) == identityProcess,
              check.executableURL?.resolvingSymlinksInPath().path == fixture.path,
              let checkWindow = try? verifiedWindow(pid: pid, title: title, reportedFrame: reportedFrame),
              checkWindow.globalID == cgWindow.globalID, checkWindow.frame == frame,
              !FileManager.default.fileExists(atPath: downPath.path),
              !FileManager.default.fileExists(atPath: upPath.path),
              ProcessInfo.processInfo.systemUptime < deadline else {
            throw ProbeError.reason("fixture or sampled state changed before posting")
        }
        // These are the only posts. They address the verified child PID, never the session.
        guard let source = CGEventSource(stateID: .privateState) else { throw ProbeError.reason("private event source unavailable") }
        source.localEventsSuppressionInterval = 0
        let permitAll: CGEventFilterMask = [.permitLocalMouseEvents, .permitLocalKeyboardEvents, .permitSystemDefinedEvents]
        source.setLocalEventsFilterDuringSuppressionState(permitAll, state: .eventSuppressionStateSuppressionInterval)
        source.setLocalEventsFilterDuringSuppressionState(permitAll, state: .eventSuppressionStateRemoteMouseDrag)
        guard source.localEventsSuppressionInterval == 0,
              source.getLocalEventsFilterDuringSuppressionState(.eventSuppressionStateSuppressionInterval).contains(permitAll),
              source.getLocalEventsFilterDuringSuppressionState(.eventSuppressionStateRemoteMouseDrag).contains(permitAll) else {
            throw ProbeError.reason("event source filter readback failed")
        }
        down.setSource(source); up.setSource(source)
        guard NSEvent(cgEvent: down)?.windowNumber == window, NSEvent(cgEvent: up)?.windowNumber == window else {
            throw ProbeError.reason("event window identity changed with source")
        }
        (samples, errors) = monitor.snapshot()
        let beforePost = ProcessInfo.processInfo.systemUptime
        guard samplesUnchanged(samples, errors, excluding: pid), let last = samples.last,
              isFresh(last, at: beforePost), child.isRunning,
              let check = NSRunningApplication(processIdentifier: pid), (try? processIdentity(pid)) == identityProcess,
              check.executableURL?.resolvingSymlinksInPath().path == fixture.path,
              let checkWindow = try? verifiedWindow(pid: pid, title: title, reportedFrame: reportedFrame),
              checkWindow.globalID == cgWindow.globalID, checkWindow.frame == frame,
              ProcessInfo.processInfo.systemUptime < deadline, isFresh(last, at: ProcessInfo.processInfo.systemUptime) else {
            throw ProbeError.reason("sample or fixture changed during event setup")
        }
        let (lastSamples, lastErrors) = monitor.snapshot()
        let readyAt = ProcessInfo.processInfo.systemUptime
        guard samplesUnchanged(lastSamples, lastErrors, excluding: pid), let fresh = lastSamples.last,
              fresh.sameState(last), isFresh(fresh, at: readyAt), child.isRunning else {
            throw ProbeError.reason("sample changed immediately before post")
        }
        result["action_attempted"] = true
        postStarted = ProcessInfo.processInfo.systemUptime
        down.postToPid(pid)
        Thread.sleep(forTimeInterval: 0.05)
        up.postToPid(pid)
        postFinished = ProcessInfo.processInfo.systemUptime
        Thread.sleep(forTimeInterval: 0.35)
        result["status"] = "experiment_completed"
        result["appkit_window_number"] = window
        result["cg_window_id"] = cgWindow.globalID
        result["target_quartz_point"] = ["x": quartzPoint.x, "y": quartzPoint.y]
        result["fixture_received_one_pair"] = record(downPath, kind: "down", pid: pid, window: window) &&
            record(upPath, kind: "up", pid: pid, window: window) &&
            !FileManager.default.fileExists(atPath: dir.appendingPathComponent("down-2.json").path) &&
            !FileManager.default.fileExists(atPath: dir.appendingPathComponent("up-2.json").path)
    } catch {
        if observeOnly, case ProbeError.reason("observation complete") = error {
            result["status"] = "observation_finished"
        } else { result["reason"] = String(describing: error) }
    }
    monitor.setPhase("teardown")
    if let child = launched, child.isRunning {
        child.terminate()
        let deadline = ProcessInfo.processInfo.systemUptime + 1
        while child.isRunning && ProcessInfo.processInfo.systemUptime < deadline { Thread.sleep(forTimeInterval: 0.02) }
    }
    let stopped = launched == nil || !(launched?.isRunning ?? true)
    if stopped, launched != nil, result["reason"] as? String == "fixture process identity not verified" {
        let errorURL = dir.appendingPathComponent("fixture-stderr.txt")
        if let attrs = try? FileManager.default.attributesOfItem(atPath: errorURL.path),
           let size = attrs[.size] as? NSNumber, size.intValue <= 512,
           let value = try? String(contentsOf: errorURL, encoding: .utf8) {
            let reason = value.trimmingCharacters(in: .whitespacesAndNewlines)
            let allowed = ["activation_policy_set_failed", "activation_policy_not_accessory", "main_screen_unavailable"]
            result["fixture_failure_category"] = allowed.contains(reason) ? reason : "other_or_empty"
        }
    }
    if stopped {
        do { try FileManager.default.removeItem(at: dir); result["cleanup_status"] = "completed" }
        catch { result["cleanup_status"] = "failed" }
    } else { result["cleanup_status"] = "not_attempted_child_running" }
    result["fixture_stopped"] = stopped
    Thread.sleep(forTimeInterval: 0.15)
    let (samples, errors, errorKinds) = monitor.detailedSnapshot()
    result["sampling_error_categories"] = errorKinds
    result["sampling_error_phases"] = monitor.phaseSnapshot()
    result["partial_diagnostics"] = monitor.partialDiagnostics(excluding: launched?.processIdentifier)
    let covered = coversPost(samples, started: postStarted, finished: postFinished)
    result["post_sampling_coverage"] = covered
    result["sampled_state_unchanged"] = samplesUnchanged(samples, errors, excluding: launched?.processIdentifier) && covered
    result["sample_count"] = samples.count
    result["sample_diagnostics"] = sampleDiagnostics(samples, errors, excluding: launched?.processIdentifier)
    result["sampling_error_count"] = errors
    if observeOnly && result["status"] as? String == "observation_finished" {
        let stable = samplesUnchanged(samples, errors, excluding: launched?.processIdentifier)
        let hiddenWindowSeen = hidden && samples.contains { sample in
            sample.windowOrder.contains { $0.owner == launched?.processIdentifier }
        }
        if hidden { result["hidden_window_seen"] = hiddenWindowSeen }
        result["observation_sampled_state_unchanged"] = stable
        if !stable || hiddenWindowSeen || !stopped || result["cleanup_status"] as? String != "completed" {
            result["status"] = "error"
        }
    }
    if result["status"] as? String == "experiment_completed" &&
       (result["fixture_received_one_pair"] as? Bool != true || result["sampled_state_unchanged"] as? Bool != true ||
        result["fixture_stopped"] as? Bool != true || result["cleanup_status"] as? String != "completed") {
        result["status"] = "error"
    }
    return result
}

@main
struct Main {
    static func main() {
        do {
            let result: [String: Any]
            switch Array(CommandLine.arguments.dropFirst()) {
            case ["--self-test"]: try selfTest(); result = ["status": "self_test_passed", "live_ui_access": false]
            case ["--observe-passive-fixture"]: throw ProbeError.reason("legacy visible fixture disabled: focus isolation unverified")
            case ["--observe-hidden-fixture"]: result = try runFixture(observeOnly: true, hidden: true)
            case ["--observe-fixture"]: throw ProbeError.reason("legacy visible fixture disabled: focus isolation unverified")
            case ["--run-fixture"]: throw ProbeError.reason("legacy visible fixture disabled: focus isolation unverified")
            default: throw ProbeError.reason("use --self-test, --observe-passive-fixture, --observe-hidden-fixture, --observe-fixture, or explicitly --run-fixture")
            }
            let bytes = try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys])
            FileHandle.standardOutput.write(bytes); FileHandle.standardOutput.write(Data([10]))
            if result["status"] as? String == "error" || result["status"] as? String == "stopped" ||
                (result["status"] as? String == "experiment_completed" &&
                 (result["fixture_received_one_pair"] as? Bool != true ||
                  result["sampled_state_unchanged"] as? Bool != true ||
                  result["fixture_stopped"] as? Bool != true ||
                  result["cleanup_status"] as? String != "completed")) { exit(3) }
        } catch {
            let bytes = try! JSONSerialization.data(withJSONObject: ["status": "stopped", "reason": String(describing: error), "general_input_authorized": false])
            FileHandle.standardError.write(bytes); FileHandle.standardError.write(Data([10])); exit(1)
        }
    }
}
