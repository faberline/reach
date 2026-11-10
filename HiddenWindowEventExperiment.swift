// Explicit own-child hidden AppKit experiment. Never accepts an external target PID or window.
// --self-test is pure data; --run-experiment is NOT run by the build script.
import AppKit
import ApplicationServices
import Darwin
import Foundation

private enum Failure: Error, CustomStringConvertible {
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
    guard pid > 0 else { throw Failure.reason("invalid process") }
    var mib = [CTL_KERN, KERN_PROC, KERN_PROC_PID, pid]
    var info = kinfo_proc()
    var size = MemoryLayout<kinfo_proc>.size
    guard sysctl(&mib, 4, &info, &size, nil, 0) == 0,
          size == MemoryLayout<kinfo_proc>.size, info.kp_proc.p_pid == pid else {
        throw Failure.reason("process identity unavailable")
    }
    var path = [CChar](repeating: 0, count: 4 * Int(MAXPATHLEN))
    guard proc_pidpath(pid, &path, UInt32(path.count)) > 0 else {
        throw Failure.reason("process executable unavailable")
    }
    let exe = URL(fileURLWithPath: String(cString: path)).resolvingSymlinksInPath().path
    let start = info.kp_proc.p_starttime
    guard start.tv_sec > 0 else { throw Failure.reason("process start unavailable") }
    return ProcessIdentity(pid: pid, parent: info.kp_eproc.e_ppid,
        startSeconds: Int64(start.tv_sec), startMicroseconds: Int32(start.tv_usec), executable: exe)
}

private struct ForegroundIdentity: Equatable {
    let pid: pid_t
    let startedSeconds: Int64
    let startedMicroseconds: Int32
    var json: [String: Any] { ["pid":pid, "started_seconds":startedSeconds, "started_microseconds":startedMicroseconds] }
    static func decode(_ value: Any) -> ForegroundIdentity? {
        guard let object = value as? [String: Any], object.count == 3,
              let pid = object["pid"] as? Int32, pid > 0,
              let seconds = object["started_seconds"] as? Int64, seconds > 0,
              let micros = object["started_microseconds"] as? Int32, (0..<1_000_000).contains(micros) else { return nil }
        return ForegroundIdentity(pid: pid, startedSeconds: seconds, startedMicroseconds: micros)
    }
}
private func frontIdentity(_ pid: pid_t) throws -> ForegroundIdentity {
    var mib = [CTL_KERN, KERN_PROC, KERN_PROC_PID, pid]
    var info = kinfo_proc()
    var size = MemoryLayout<kinfo_proc>.size
    guard pid > 0, sysctl(&mib, 4, &info, &size, nil, 0) == 0, size == MemoryLayout<kinfo_proc>.size,
          info.kp_proc.p_pid == pid, info.kp_proc.p_starttime.tv_sec > 0,
          (0..<1_000_000).contains(info.kp_proc.p_starttime.tv_usec) else {
        throw Failure.reason("cannot sample foreground identity")
    }
    return ForegroundIdentity(pid: pid, startedSeconds: Int64(info.kp_proc.p_starttime.tv_sec),
                              startedMicroseconds: Int32(info.kp_proc.p_starttime.tv_usec))
}

private struct Sample: Equatable {
    let started: Double
    let finished: Double
    let front: pid_t
    let foregroundIdentity: ForegroundIdentity
    let focused: pid_t
    let focusedHash: CFHashCode
    let x: Double
    let y: Double
    let mouseButtonsDown: Bool
    let keyboardKeysDown: Bool
    let modifierFlags: UInt64
    let windowOrder: [UInt32]
    func sameState(_ other: Sample) -> Bool {
        front == other.front && foregroundIdentity == other.foregroundIdentity && focused == other.focused && focusedHash == other.focusedHash &&
        x == other.x && y == other.y && mouseButtonsDown == other.mouseButtonsDown && keyboardKeysDown == other.keyboardKeysDown && modifierFlags == other.modifierFlags &&
        windowOrder == other.windowOrder
    }
}

// Private samples stay in memory. The result never stores user cursor positions.
private func sample() throws -> Sample {
    let started = ProcessInfo.processInfo.systemUptime
    guard let app = NSWorkspace.shared.frontmostApplication, let event = CGEvent(source: nil) else {
        throw Failure.reason("cannot sample frontmost app or pointer")
    }
    guard let windows = CGWindowListCopyWindowInfo(.optionOnScreenOnly, kCGNullWindowID) as? [[String: Any]] else {
        throw Failure.reason("cannot sample window order")
    }
    let order = windows.compactMap { ($0[kCGWindowNumber as String] as? NSNumber)?.uint32Value }
    guard !order.isEmpty, order.count == windows.count, order.count <= 4096,
          !order.contains(0), Set(order).count == order.count else {
        throw Failure.reason("cannot sample window order")
    }
    let system = AXUIElementCreateSystemWide()
    _ = AXUIElementSetMessagingTimeout(system, 0.2)
    func focusedApplication() throws -> (element: AXUIElement, pid: pid_t) {
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(system, kAXFocusedApplicationAttribute as CFString, &value) == .success,
              let value, CFGetTypeID(value) == AXUIElementGetTypeID() else {
            throw Failure.reason("cannot sample focused application")
        }
        let element = unsafeBitCast(value, to: AXUIElement.self)
        var pid: pid_t = 0
        guard AXUIElementGetPid(element, &pid) == .success, pid > 0 else {
            throw Failure.reason("cannot sample focused application")
        }
        return (element, pid)
    }
    let focusBefore = try focusedApplication()
    var raw: CFTypeRef?
    guard AXUIElementCopyAttributeValue(system, kAXFocusedUIElementAttribute as CFString, &raw) == .success,
          let value = raw, CFGetTypeID(value) == AXUIElementGetTypeID() else {
        throw Failure.reason("cannot sample focused element")
    }
    let element = unsafeBitCast(value, to: AXUIElement.self)
    var pid: pid_t = 0
    guard AXUIElementGetPid(element, &pid) == .success, pid > 0,
          event.location.x.isFinite, event.location.y.isFinite else { throw Failure.reason("invalid sample") }
    let focusAfter = try focusedApplication()
    guard focusSourcesAgree(front: app.processIdentifier, before: focusBefore.pid, after: focusAfter.pid,
                            element: pid, sameApplication: CFEqual(focusBefore.element, focusAfter.element)) else {
        throw Failure.reason("focus sources disagree")
    }
    let buttons = [CGMouseButton.left, .right, .center].contains { CGEventSource.buttonState(.hidSystemState, button: $0) }
    // Only retain an aggregate. Never store key codes or typed text.
    let keys = (0..<128).contains { CGEventSource.keyState(.hidSystemState, key: CGKeyCode($0)) }
    let flags = CGEventSource.flagsState(.hidSystemState).rawValue
    let identity = try frontIdentity(app.processIdentifier)
    guard NSWorkspace.shared.frontmostApplication?.processIdentifier == identity.pid else {
        throw Failure.reason("foreground changed during sample")
    }
    return Sample(started: started, finished: ProcessInfo.processInfo.systemUptime,
                  front: app.processIdentifier, foregroundIdentity: identity, focused: pid, focusedHash: CFHash(element),
                  x: event.location.x, y: event.location.y, mouseButtonsDown: buttons, keyboardKeysDown: keys, modifierFlags: flags, windowOrder: order)
}

private func focusSourcesAgree(front: pid_t, before: pid_t, after: pid_t, element: pid_t, sameApplication: Bool) -> Bool {
    front > 0 && before == front && after == front && element == front && sameApplication
}

private final class Monitor {
    private let lock = NSLock()
    private var values: [Sample] = []
    private var failures = 0
    private var failureKinds: [String: Int] = [:]
    private let timer = DispatchSource.makeTimerSource(queue: DispatchQueue(label: "hidden-window-monitor"))
    func start() {
        timer.schedule(deadline: .now(), repeating: .milliseconds(20))
        timer.setEventHandler { [weak self] in
            guard let self else { return }
            do { let value = try sample(); self.lock.lock(); self.values.append(value); self.lock.unlock() }
            catch {
                let kind = sampleFailureKind(error)
                self.lock.lock(); self.failures += 1; self.failureKinds[kind, default: 0] += 1; self.lock.unlock()
            }
        }
        timer.resume()
    }
    func snapshot() -> ([Sample], Int) { lock.lock(); defer { lock.unlock() }; return (values, failures) }
    func detailedSnapshot() -> ([Sample], Int, [String: Int]) {
        lock.lock(); defer { lock.unlock() }; return (values, failures, failureKinds)
    }
    func stop() { timer.cancel() }
}

// Only fixed categories leave the process; never include window names or positions.
private func sampleFailureKind(_ error: Error) -> String {
    switch error {
    case Failure.reason("cannot sample focused element"): return "focus_unavailable"
    case Failure.reason("cannot sample focused application"): return "focused_application_unavailable"
    case Failure.reason("focus sources disagree"): return "focus_sources_disagree"
    case Failure.reason("cannot sample frontmost app or pointer"): return "foreground_or_pointer_unavailable"
    case Failure.reason("cannot sample window order"): return "window_order_unavailable"
    case Failure.reason("invalid sample"): return "invalid_sample"
    case Failure.reason("cannot sample foreground identity"): return "foreground_identity_unavailable"
    case Failure.reason("foreground changed during sample"): return "foreground_changed"
    default: return "other"
    }
}

private func samplesUnchanged(_ values: [Sample], _ failures: Int) -> Bool {
    guard let first = values.first, values.count >= 3, failures == 0 else { return false }
    var previousFinished: Double?
    for value in values {
        guard value.started.isFinite, value.finished.isFinite, value.started >= 0,
              value.finished >= value.started, value.finished - value.started <= 0.25,
              !value.mouseButtonsDown, !value.keyboardKeysDown, value.modifierFlags == 0,
              value.sameState(first) else { return false }
        if let previousFinished {
            guard value.started >= previousFinished, value.started - previousFinished <= 0.25 else { return false }
        }
        previousFinished = value.finished
    }
    return true
}

// Aggregates only; no app identities, key values, cursor coordinates or window IDs leave the process.
private func sampleDiagnostics(_ values: [Sample], _ failures: Int) -> [String: Any] {
    guard failures == 0, values.count >= 3, let first = values.first else {
        return ["available": false]
    }
    return ["available": true,
        "frontmost_unchanged": values.allSatisfy { $0.front == first.front },
        "focused_element_unchanged": values.allSatisfy { $0.focused == first.focused && $0.focusedHash == first.focusedHash },
        "cursor_unchanged": values.allSatisfy { $0.x == first.x && $0.y == first.y },
        "window_order_unchanged": values.allSatisfy { $0.windowOrder == first.windowOrder },
        "mouse_buttons_clear": values.allSatisfy { !$0.mouseButtonsDown },
        "keyboard_keys_clear": values.allSatisfy { !$0.keyboardKeysDown },
        "modifier_flags_clear": values.allSatisfy { $0.modifierFlags == 0 }]
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

private func validReceipt(_ value: [String: Any], child: pid_t, parent: pid_t,
                          nonce: String, marker: Int64) -> Bool {
    value["nonce"] as? String == nonce && value["child_pid"] as? Int32 == child &&
    value["sender_pid"] as? Int32 == parent && value["marker"] as? Int64 == marker &&
    value["kind"] as? String == "mouse_moved" && value["count"] as? Int == 1
}

private func validFinalCount(_ value: [String: Any], child: pid_t, nonce: String, marker: Int64) -> Bool {
    value["nonce"] as? String == nonce && value["child_pid"] as? Int32 == child &&
    value["marker"] as? Int64 == marker && value["count"] as? Int == 1
}

private func validParentHandshake(_ value: [String: Any], parent: ProcessIdentity,
                                  nonce: String, marker: Int64) -> Bool {
    value["purpose"] as? String == "hidden_window_experiment" &&
    value["nonce"] as? String == nonce && value["marker"] as? Int64 == marker &&
    value["parent_pid"] as? Int32 == parent.pid &&
    value["parent_start_seconds"] as? Int64 == parent.startSeconds &&
    value["parent_start_microseconds"] as? Int32 == parent.startMicroseconds &&
    value["parent_executable"] as? String == parent.executable
}

// Report cleanup even when it fails. Never remove files while the child is live.
private func cleanup(stopped: Bool, remove: () throws -> Void) -> (status: String, error: String?) {
    guard stopped else { return ("not_attempted_child_running", "child still running") }
    do { try remove(); return ("completed", nil) }
    catch { return ("failed", String(describing: error)) }
}

private func retainTrialEvidence(actionAttempted: Bool, status: String?, tapReceipt: Bool,
                                 appkitReceipt: Bool, viewReceipt: Bool, stable: Bool, covered: Bool, stopped: Bool) -> Bool {
    actionAttempted && (status != "experiment_completed" || !tapReceipt || !appkitReceipt || !viewReceipt || !stable || !covered || !stopped)
}

private func completedSuccessfully(_ value: [String: Any]) -> Bool {
    value["status"] as? String == "experiment_completed" &&
    value["child_received_one_tagged_event_in_observation_window"] as? Bool == true &&
    value["appkit_received_one_tagged_event"] as? Bool == true &&
    value["view_received_one_tagged_event"] as? Bool == true &&
    value["sampled_state_unchanged"] as? Bool == true &&
    value["child_stopped"] as? Bool == true && value["cleanup_status"] as? String == "completed"
}

private func writeNew(_ object: [String: Any], to path: URL) throws {
    let data = try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
    try data.write(to: path, options: .withoutOverwriting)
}

private final class Receiver {
    let directory: URL
    let parent: pid_t
    let nonce: String
    let marker: Int64
    var count = 0
    init(directory: URL, parent: pid_t, nonce: String, marker: Int64) {
        self.directory = directory; self.parent = parent; self.nonce = nonce; self.marker = marker
    }
    func receive(_ type: CGEventType, _ event: CGEvent) {
        guard type == .mouseMoved, event.getIntegerValueField(.eventSourceUserData) == marker,
              event.getIntegerValueField(.eventSourceUnixProcessID) == Int64(parent) else { return }
        count += 1
        let receipt: [String: Any] = ["nonce": nonce, "child_pid": getpid(), "sender_pid": parent,
            "marker": marker, "kind": "mouse_moved", "count": count,
            "event_timestamp": event.timestamp]
        try? writeNew(receipt, to: directory.appendingPathComponent("event-\(count).json"))
    }
}

private func tapCallback(_ proxy: CGEventTapProxy, _ type: CGEventType, _ event: CGEvent,
                         _ userInfo: UnsafeMutableRawPointer?) -> Unmanaged<CGEvent>? {
    if let userInfo { Unmanaged<Receiver>.fromOpaque(userInfo).takeUnretainedValue().receive(type, event) }
    return Unmanaged.passUnretained(event) // listen-only: never modify or suppress an event.
}

// Diagnostic only: never persist the pointer or event coordinates.
private func pointInside(_ point: NSPoint, bounds: NSRect) -> Bool? {
    guard point.x.isFinite, point.y.isFinite, bounds.origin.x.isFinite, bounds.origin.y.isFinite,
          bounds.width.isFinite, bounds.height.isFinite, bounds.width > 0, bounds.height > 0 else { return nil }
    return bounds.contains(point)
}

private final class HiddenView: NSView {
    let directory: URL; let parent: pid_t; let nonce: String; let marker: Int64
    var count = 0
    init(directory: URL, parent: pid_t, nonce: String, marker: Int64) {
        self.directory = directory; self.parent = parent; self.nonce = nonce; self.marker = marker
        super.init(frame: NSRect(x: 0, y: 0, width: 200, height: 100))
    }
    required init?(coder: NSCoder) { nil }
    override func mouseMoved(with event: NSEvent) {
        guard let cg = event.cgEvent, cg.getIntegerValueField(.eventSourceUserData) == marker,
              cg.getIntegerValueField(.eventSourceUnixProcessID) == Int64(parent) else { return }
        count += 1
        try? writeNew(["nonce":nonce, "child_pid":getpid(), "sender_pid":parent, "marker":marker,
                       "kind":"view_mouse_moved", "count":count, "window_number":event.windowNumber],
                      to:directory.appendingPathComponent("view-\(count).json"))
    }
}

private func child(_ args: [String]) throws {
    guard args.count == 4, args[0] == "--child", let parent = Int32(args[1]), parent == getppid(),
          let uuid = UUID(uuidString: args[2]), uuid.uuidString == args[2],
          let marker = Int64(args[3]), marker > 0,
          let rawDir = ProcessInfo.processInfo.environment["HIDDEN_WINDOW_PRIVATE_DIR"] else {
        throw Failure.reason("invalid child arguments")
    }
    let dir = URL(fileURLWithPath: rawDir, isDirectory: true).standardizedFileURL
    let expected = URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
        .appendingPathComponent("hidden-window-\(uuid.uuidString)").standardizedFileURL
    guard dir == expected else { throw Failure.reason("child scope mismatch") }
    let own = try processIdentity(getpid())
    let parentIdentity = try processIdentity(parent)
    guard own.parent == parent, parentIdentity.executable == own.executable else {
        throw Failure.reason("parent and child executables differ")
    }
    // The controlled parent creates this private directory and one-time handshake
    // before launching the child. Arbitrary --child invocations fail before a tap.
    var dirInfo = stat()
    guard lstat(dir.path, &dirInfo) == 0, (dirInfo.st_mode & S_IFMT) == S_IFDIR,
          dirInfo.st_uid == getuid(), (dirInfo.st_mode & 0o077) == 0 else {
        throw Failure.reason("private child directory unavailable")
    }
    let handshakePath = dir.appendingPathComponent("parent-handshake.json")
    var fileInfo = stat()
    guard lstat(handshakePath.path, &fileInfo) == 0, (fileInfo.st_mode & S_IFMT) == S_IFREG,
          fileInfo.st_uid == getuid(), let data = try? Data(contentsOf: handshakePath),
          let handshake = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
          validParentHandshake(handshake, parent: parentIdentity, nonce: uuid.uuidString, marker: marker),
          getppid() == parent, try processIdentity(parent) == parentIdentity else {
        throw Failure.reason("controlled parent handshake unavailable")
    }
    guard CGPreflightListenEventAccess() else { throw Failure.reason("listen permission unavailable") }
    let app = NSApplication.shared
    if app.activationPolicy() != .accessory { _ = app.setActivationPolicy(.accessory) }
    guard app.activationPolicy() == .accessory else { throw Failure.reason("child_activation_policy_not_accessory") }
    let window = NSPanel(contentRect: NSRect(x: 0, y: 0, width: 200, height: 100),
                         styleMask: [.titled, .nonactivatingPanel], backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.acceptsMouseMovedEvents = true
    let view = HiddenView(directory: dir, parent: parent, nonce: uuid.uuidString, marker: marker)
    window.contentView = view
    guard !window.isVisible, !window.isKeyWindow, window.windowNumber > 0 else { throw Failure.reason("child_window_not_hidden") }
    var appkitCount = 0
    guard let monitor = NSEvent.addLocalMonitorForEvents(matching: .mouseMoved, handler: { event in
        guard let cg = event.cgEvent, cg.getIntegerValueField(.eventSourceUserData) == marker,
              cg.getIntegerValueField(.eventSourceUnixProcessID) == Int64(parent) else { return event }
        appkitCount += 1
        // Compare only within this controlled window. An unrelated window or
        // invalid coordinates is unknown, never a negative hit-test result.
        let inside: Bool? = event.windowNumber == window.windowNumber
            ? pointInside(view.convert(event.locationInWindow, from: nil), bounds: view.bounds) : nil
        try? writeNew(["nonce":uuid.uuidString, "child_pid":getpid(), "sender_pid":parent,
                       "marker":marker, "kind":"appkit_mouse_moved", "count":appkitCount,
                       "window_number":event.windowNumber, "event_inside_view_bounds":inside as Any? ?? NSNull(),
                       "view_tracking_area_count":view.trackingAreas.count], to:dir.appendingPathComponent("appkit-\(appkitCount).json"))
        return event
    }) else { throw Failure.reason("appkit_monitor_unavailable") }
    let receiver = Receiver(directory: dir, parent: parent, nonce: uuid.uuidString, marker: marker)
    let mask = CGEventMask(1) << CGEventType.mouseMoved.rawValue
    guard getppid() == parent, try processIdentity(parent) == parentIdentity else {
        throw Failure.reason("parent changed before tap")
    }
    guard let tap = CGEvent.tapCreateForPid(pid: getpid(), place: .headInsertEventTap,
        options: .listenOnly, eventsOfInterest: mask, callback: tapCallback,
        userInfo: Unmanaged.passUnretained(receiver).toOpaque()),
        let source = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, tap, 0) else {
        throw Failure.reason("cannot create child-only listen tap")
    }
    CFRunLoopAddSource(CFRunLoopGetCurrent(), source, .defaultMode)
    CGEvent.tapEnable(tap: tap, enable: true)
    guard CGEvent.tapIsEnabled(tap: tap), getppid() == parent else { throw Failure.reason("tap or parent unavailable") }
    let deadline = ProcessInfo.processInfo.systemUptime + 8
    var drainUntil: Double?
    var stopping = false
    let timer = Timer(timeInterval: 0.05, repeats: true) { _ in
        if FileManager.default.fileExists(atPath: dir.appendingPathComponent("done.json").path), drainUntil == nil {
            drainUntil = ProcessInfo.processInfo.systemUptime + 0.35
        }
        if !stopping && (getppid() != parent || window.isVisible || receiver.count >= 2 || appkitCount >= 2 ||
           ProcessInfo.processInfo.systemUptime >= deadline || (drainUntil.map { ProcessInfo.processInfo.systemUptime >= $0 } ?? false)) {
            stopping = true
            app.stop(nil)
            // Wake only this child's AppKit queue; this is not a mouse or keyboard event.
            if let wake = NSEvent.otherEvent(with: .applicationDefined, location: .zero, modifierFlags: [],
                timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: 0, context: nil,
                subtype: 0, data1: 0, data2: 0) { app.postEvent(wake, atStart: false) }
        }
    }
    RunLoop.main.add(timer, forMode: .default)
    DispatchQueue.main.async {
        try? writeNew(["nonce": uuid.uuidString, "child_pid": getpid(), "parent_pid": parent,
            "tap_enabled": true, "activation_policy": "accessory", "window_hidden": !window.isVisible, "window_number":window.windowNumber],
            to: dir.appendingPathComponent("ready.json"))
    }
    app.run()
    timer.invalidate()
    NSEvent.removeMonitor(monitor)
    CGEvent.tapEnable(tap: tap, enable: false)
    CFRunLoopRemoveSource(CFRunLoopGetCurrent(), source, .defaultMode)
    CFMachPortInvalidate(tap)
    // This final count is required even if an individual receipt write failed.
    try writeNew(["nonce": uuid.uuidString, "child_pid": getpid(), "marker": marker,
        "count": receiver.count, "appkit_count":appkitCount, "view_count":view.count,
        "window_number":window.windowNumber, "window_hidden":!window.isVisible], to: dir.appendingPathComponent("final-count.json"))
    withExtendedLifetime(receiver) {}
}

private func selfTest() throws {
    let rect = NSRect(x: 0, y: 0, width: 200, height: 100)
    guard pointInside(NSPoint(x: 1, y: 1), bounds: rect) == true,
          pointInside(NSPoint(x: 201, y: 1), bounds: rect) == false,
          pointInside(NSPoint(x: CGFloat.nan, y: 1), bounds: rect) == nil,
          pointInside(NSPoint(x: 1, y: 1), bounds: .zero) == nil else {
        throw Failure.reason("view bounds diagnostic self-test failed")
    }
    guard !retainTrialEvidence(actionAttempted: false, status: "stopped", tapReceipt: false, appkitReceipt: false, viewReceipt: false, stable: false, covered: false, stopped: true),
          !retainTrialEvidence(actionAttempted: true, status: "experiment_completed", tapReceipt: true, appkitReceipt: true, viewReceipt: true, stable: true, covered: true, stopped: true),
          retainTrialEvidence(actionAttempted: true, status: "stopped", tapReceipt: false, appkitReceipt: false, viewReceipt: false, stable: true, covered: false, stopped: true),
          retainTrialEvidence(actionAttempted: true, status: "experiment_completed", tapReceipt: true, appkitReceipt: false, viewReceipt: false, stable: true, covered: true, stopped: true),
          retainTrialEvidence(actionAttempted: true, status: "experiment_completed", tapReceipt: true, appkitReceipt: true, viewReceipt: true, stable: false, covered: true, stopped: true) else {
        throw Failure.reason("evidence retention self-test failed")
    }
    let expected = ForegroundIdentity(pid: 1, startedSeconds: 10, startedMicroseconds: 20)
    guard ForegroundIdentity.decode(expected.json) == expected,
          ForegroundIdentity.decode(["pid":1, "started_seconds":10, "started_microseconds":1_000_000]) == nil,
          ForegroundIdentity.decode(["pid":1, "started_seconds":10, "started_microseconds":20, "extra":1]) == nil else {
        throw Failure.reason("foreground identity self-test failed")
    }
    let known = Data("{\"reason\":\"child_activation_policy_not_accessory\"}".utf8)
    let unknown = Data("{\"reason\":\"private data\"}".utf8)
    guard childFailureCategory(known) == "child_activation_policy_not_accessory",
          childFailureCategory(unknown) == "other_or_unreadable",
          childFailureCategory(Data(repeating: 65, count: 4097)) == "other_or_unreadable" else {
        throw Failure.reason("child diagnostic self-test failed")
    }
    guard focusSourcesAgree(front: 10, before: 10, after: 10, element: 10, sameApplication: true),
          !focusSourcesAgree(front: 10, before: 11, after: 10, element: 10, sameApplication: true),
          !focusSourcesAgree(front: 10, before: 10, after: 11, element: 10, sameApplication: true),
          !focusSourcesAgree(front: 10, before: 10, after: 10, element: 11, sameApplication: true),
          !focusSourcesAgree(front: 10, before: 10, after: 10, element: 10, sameApplication: false),
          !focusSourcesAgree(front: 0, before: 0, after: 0, element: 0, sameApplication: true) else {
        throw Failure.reason("focus source check self-test failed")
    }
    guard sampleFailureKind(Failure.reason("cannot sample focused element")) == "focus_unavailable",
          sampleFailureKind(Failure.reason("cannot sample focused application")) == "focused_application_unavailable",
          sampleFailureKind(Failure.reason("focus sources disagree")) == "focus_sources_disagree",
          sampleFailureKind(Failure.reason("cannot sample frontmost app or pointer")) == "foreground_or_pointer_unavailable",
          sampleFailureKind(Failure.reason("cannot sample window order")) == "window_order_unavailable",
          sampleFailureKind(Failure.reason("invalid sample")) == "invalid_sample",
          sampleFailureKind(Failure.reason("arbitrary private data")) == "other" else {
        throw Failure.reason("sample failure categorization failed")
    }
    func sampleData(_ t: Double, _ x: Double = 3, buttons: Bool = false, keys: Bool = false, flags: UInt64 = 0, order: [UInt32] = [1,2]) -> Sample {
        Sample(started: t, finished: t + 0.01, front: 1,
               foregroundIdentity: ForegroundIdentity(pid: 1, startedSeconds: 10, startedMicroseconds: 20), focused: 1, focusedHash: 2, x: x, y: 4,
               mouseButtonsDown: buttons, keyboardKeysDown: keys, modifierFlags: flags, windowOrder: order)
    }
    let a = sampleData(1), b = sampleData(1.02), c = sampleData(1.04)
    guard sampleDiagnostics([a,b,c], 0)["available"] as? Bool == true,
          sampleDiagnostics([a,b,sampleData(1.04, order: [2,1])], 0)["window_order_unchanged"] as? Bool == false,
          sampleDiagnostics([a,b,sampleData(1.04, buttons: true)], 0)["mouse_buttons_clear"] as? Bool == false,
          sampleDiagnostics([a,b,sampleData(1.04, keys: true)], 0)["keyboard_keys_clear"] as? Bool == false,
          sampleDiagnostics([a,b], 0)["available"] as? Bool == false else {
        throw Failure.reason("sample diagnostics self-test failed")
    }
    let postSamples = [a, b, c, sampleData(1.08), sampleData(1.10), sampleData(1.12)]
    guard samplesUnchanged([a,b,c], 0), !samplesUnchanged([a,b], 0),
          !samplesUnchanged([a,b,sampleData(1.04, 9)], 0), !samplesUnchanged([a,b,c], 1),
          !samplesUnchanged([a,b,sampleData(1.04, buttons: true)], 0),
          !samplesUnchanged([a,b,sampleData(1.04, keys: true)], 0),
          !samplesUnchanged([a,b,sampleData(1.04, flags: 1)], 0),
          !samplesUnchanged([a,b,sampleData(1.04, order: [2,1])], 0),
          !samplesUnchanged([sampleData(1, buttons: true),sampleData(1.02, buttons: true),sampleData(1.04, buttons: true)], 0),
          !samplesUnchanged([sampleData(1, flags: 1),sampleData(1.02, flags: 1),sampleData(1.04, flags: 1)], 0),
          !samplesUnchanged([a,b,sampleData(2)], 0), !samplesUnchanged([a,c,b], 0),
          coversPost(postSamples, started: 1.055, finished: 1.065),
          !coversPost([a,b,c], started: 1.055, finished: 1.065),
          !coversPost(Array(postSamples.prefix(5)), started: 1.055, finished: 1.065),
          !coversPost(postSamples, started: 0.5, finished: 0.51),
          !coversPost(postSamples, started: 1.4, finished: 1.41),
          !coversPost(postSamples, started: 1.055, finished: 1.5),
          !coversPost(postSamples, started: nil, finished: 1.065),
          !coversPost(postSamples, started: 1.065, finished: 1.055),
          isFresh(c, at: 1.06), !isFresh(c, at: 1.04), !isFresh(c, at: 1.4),
          !isFresh(c, at: .nan) else {
        throw Failure.reason("sample self-test failed")
    }
    let good: [String: Any] = ["nonce":"nonce", "child_pid":Int32(42), "sender_pid":Int32(3),
                               "marker":Int64(99), "kind":"mouse_moved", "count":1]
    let roundtrip = try JSONSerialization.jsonObject(with: JSONSerialization.data(withJSONObject: good)) as? [String: Any]
    guard validReceipt(good, child: 42, parent: 3, nonce: "nonce", marker: 99),
          roundtrip.map({ validReceipt($0, child: 42, parent: 3, nonce: "nonce", marker: 99) }) == true,
          !validReceipt(good, child: 41, parent: 3, nonce: "nonce", marker: 99),
          !validReceipt(good, child: 42, parent: 4, nonce: "nonce", marker: 99),
          !validReceipt(good, child: 42, parent: 3, nonce: "other", marker: 99),
          !validReceipt(good, child: 42, parent: 3, nonce: "nonce", marker: 98) else {
        throw Failure.reason("receipt self-test failed")
    }
    let finalCount: [String: Any] = ["nonce":"nonce", "child_pid":Int32(42), "marker":Int64(99), "count":1]
    var duplicateCount = finalCount; duplicateCount["count"] = 2
    guard validFinalCount(finalCount, child: 42, nonce: "nonce", marker: 99),
          !validFinalCount(duplicateCount, child: 42, nonce: "nonce", marker: 99),
          !validFinalCount(finalCount, child: 41, nonce: "nonce", marker: 99),
          !validFinalCount(finalCount, child: 42, nonce: "other", marker: 99),
          !validFinalCount(finalCount, child: 42, nonce: "nonce", marker: 98) else {
        throw Failure.reason("final count self-test failed")
    }
    let parent = ProcessIdentity(pid: 3, parent: 2, startSeconds: 100, startMicroseconds: 9, executable: "/parent")
    let handshake: [String: Any] = ["purpose":"hidden_window_experiment", "nonce":"nonce", "marker":Int64(99),
        "parent_pid":Int32(3), "parent_start_seconds":Int64(100), "parent_start_microseconds":Int32(9), "parent_executable":"/parent"]
    let decoded = try JSONSerialization.jsonObject(with: JSONSerialization.data(withJSONObject: handshake)) as? [String: Any]
    var largeMarker = handshake
    largeMarker["marker"] = Int64.max
    let largeDecoded = try JSONSerialization.jsonObject(with: JSONSerialization.data(withJSONObject: largeMarker)) as? [String: Any]
    guard validParentHandshake(handshake, parent: parent, nonce: "nonce", marker: 99),
          decoded.map({ validParentHandshake($0, parent: parent, nonce: "nonce", marker: 99) }) == true,
          largeDecoded.map({ validParentHandshake($0, parent: parent, nonce: "nonce", marker: Int64.max) }) == true,
          !validParentHandshake(handshake, parent: parent, nonce: "other", marker: 99),
          !validParentHandshake(handshake, parent: parent, nonce: "nonce", marker: 98),
          !validParentHandshake(handshake, parent: ProcessIdentity(pid: 3, parent: 2, startSeconds: 101, startMicroseconds: 9, executable: "/parent"), nonce: "nonce", marker: 99) else {
        throw Failure.reason("parent handshake self-test failed")
    }
    let cleaned = cleanup(stopped: true) {}
    let failed = cleanup(stopped: true) { throw Failure.reason("forced cleanup failure") }
    var called = false
    let running = cleanup(stopped: false) { called = true }
    guard cleaned.status == "completed", failed.status == "failed", failed.error != nil,
          running.status == "not_attempted_child_running", !called else { throw Failure.reason("cleanup self-test failed") }
    var successful: [String: Any] = ["status":"experiment_completed", "child_received_one_tagged_event_in_observation_window":true, "appkit_received_one_tagged_event":true, "view_received_one_tagged_event":true,
        "sampled_state_unchanged":true, "child_stopped":true, "cleanup_status":"completed"]
    guard completedSuccessfully(successful) else { throw Failure.reason("result self-test failed") }
    successful["cleanup_status"] = "failed"
    guard !completedSuccessfully(successful) else { throw Failure.reason("cleanup failure accepted") }
}

private func childFailureCategory(_ data: Data) -> String {
    guard data.count <= 4096, let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
          let reason = object["reason"] as? String else { return "other_or_unreadable" }
    let allowed: Set<String> = ["child_activation_policy_set_failed", "child_activation_policy_not_accessory",
        "listen permission unavailable", "cannot create child-only listen tap", "tap or parent unavailable",
        "parent changed before tap", "controlled parent handshake unavailable", "parent and child executables differ",
        "private child directory unavailable", "child_window_not_hidden", "appkit_monitor_unavailable"]
    return allowed.contains(reason) ? reason : "other_or_unreadable"
}

private func runExperiment(expectedFront: ForegroundIdentity?, observeOnly: Bool) throws -> [String: Any] {
    guard AXIsProcessTrusted(), CGPreflightListenEventAccess(), CGPreflightPostEventAccess() else {
        throw Failure.reason("required permissions unavailable; no prompt requested")
    }
    let own = try processIdentity(getpid())
    let nonce = UUID().uuidString
    let marker = Int64.random(in: 1...Int64.max)
    let dir = URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true).appendingPathComponent("hidden-window-\(nonce)")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
    let monitor = Monitor(); monitor.start(); defer { monitor.stop() }
    var result: [String: Any] = ["status":"stopped", "action_attempted":false,
        "general_input_authorized":false, "experiment_scope":"own_hidden_window_child_only",
        "expected_foreground_required":expectedFront != nil,
        "note":"Only an invisible own-child window is tested. Sampling can miss brief changes. No input isolation or visible App behavior is proven."]
    var launched: Process?
    var postStarted: Double?
    var postFinished: Double?
    do {
        Thread.sleep(forTimeInterval: 0.2)
        var (values, errors) = monitor.snapshot()
        guard samplesUnchanged(values, errors), let baseline = values.first else { throw Failure.reason("no stable sampled baseline") }
        guard expectedFront == nil || baseline.foregroundIdentity == expectedFront else { throw Failure.reason("expected foreground identity mismatch") }
        try writeNew(["purpose":"hidden_window_experiment", "nonce":nonce, "marker":marker,
            "parent_pid":own.pid, "parent_start_seconds":own.startSeconds,
            "parent_start_microseconds":own.startMicroseconds, "parent_executable":own.executable],
            to: dir.appendingPathComponent("parent-handshake.json"))
        let process = Process()
        process.executableURL = URL(fileURLWithPath: own.executable)
        process.arguments = ["--child", String(getpid()), nonce, String(marker)]
        var env = ProcessInfo.processInfo.environment; env["HIDDEN_WINDOW_PRIVATE_DIR"] = dir.path
        process.environment = env
        process.standardOutput = FileHandle.nullDevice
        let errorPath = dir.appendingPathComponent("child-stderr.json")
        let errorFD = open(errorPath.path, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW, mode_t(0o600))
        guard errorFD >= 0 else { throw Failure.reason("child diagnostic setup failed") }
        let errorHandle = FileHandle(fileDescriptor: errorFD, closeOnDealloc: true)
        process.standardError = errorHandle
        do { try process.run() } catch { try? errorHandle.close(); throw error }
        try? errorHandle.close()
        launched = process
        let pid = process.processIdentifier
        let identity = try processIdentity(pid)
        guard identity.parent == getpid(), identity.executable == own.executable else { throw Failure.reason("child identity mismatch") }
        let readyPath = dir.appendingPathComponent("ready.json")
        let deadline = ProcessInfo.processInfo.systemUptime + 4
        while process.isRunning && !FileManager.default.fileExists(atPath: readyPath.path) && ProcessInfo.processInfo.systemUptime < deadline {
            Thread.sleep(forTimeInterval: 0.05)
        }
        guard process.isRunning, let data = try? Data(contentsOf: readyPath),
              let ready = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              ready["child_pid"] as? Int32 == pid, ready["parent_pid"] as? Int32 == getpid(),
              ready["nonce"] as? String == nonce, ready["tap_enabled"] as? Bool == true,
              ready["activation_policy"] as? String == "accessory", ready["window_hidden"] as? Bool == true,
              let windowNumber = ready["window_number"] as? Int, windowNumber > 0 else { throw Failure.reason("child tap readiness unavailable") }
        if observeOnly {
            try writeNew(["nonce":nonce], to: dir.appendingPathComponent("done.json"))
            let deadline = ProcessInfo.processInfo.systemUptime + 2
            while process.isRunning && ProcessInfo.processInfo.systemUptime < deadline { Thread.sleep(forTimeInterval: 0.02) }
            guard !process.isRunning else { throw Failure.reason("child did not stop during lifecycle observation") }
            let finalData = try? Data(contentsOf: dir.appendingPathComponent("final-count.json"))
            let finalCount = finalData.flatMap { (try? JSONSerialization.jsonObject(with: $0)) as? [String: Any] }
            guard process.terminationStatus == 0, let finalCount,
                  (finalCount["nonce"] as? String) == nonce, (finalCount["child_pid"] as? Int32) == pid,
                  (finalCount["count"] as? Int) == 0, (finalCount["appkit_count"] as? Int) == 0, (finalCount["view_count"] as? Int) == 0,
                  (finalCount["window_hidden"] as? Bool) == true else {
                throw Failure.reason("lifecycle final state unavailable")
            }
            result["status"] = "lifecycle_observed"
            result["lifecycle_final_count"] = finalCount
        } else {
        guard let source = CGEventSource(stateID: .privateState) else { throw Failure.reason("private event source unavailable") }
        source.localEventsSuppressionInterval = 0
        let permitAll: CGEventFilterMask = [.permitLocalMouseEvents, .permitLocalKeyboardEvents, .permitSystemDefinedEvents]
        source.setLocalEventsFilterDuringSuppressionState(permitAll, state: .eventSuppressionStateSuppressionInterval)
        source.setLocalEventsFilterDuringSuppressionState(permitAll, state: .eventSuppressionStateRemoteMouseDrag)
        guard source.localEventsSuppressionInterval == 0,
              source.getLocalEventsFilterDuringSuppressionState(.eventSuppressionStateSuppressionInterval).contains(permitAll),
              source.getLocalEventsFilterDuringSuppressionState(.eventSuppressionStateRemoteMouseDrag).contains(permitAll) else {
            throw Failure.reason("event-source filter readback failed")
        }
        (values, errors) = monitor.snapshot()
        let beforePost = ProcessInfo.processInfo.systemUptime
        guard samplesUnchanged(values, errors), let last = values.last,
              isFresh(last, at: beforePost), process.isRunning,
              try processIdentity(pid) == identity, ProcessInfo.processInfo.systemUptime < deadline,
              !FileManager.default.fileExists(atPath: dir.appendingPathComponent("event-1.json").path), NSApp == nil else {
            throw Failure.reason("child or samples changed")
        }
        let screenPoint = NSPoint(x: last.x, y: CGDisplayBounds(CGMainDisplayID()).height - last.y)
        guard let wrapped = NSEvent.mouseEvent(with: .mouseMoved, location: screenPoint, modifierFlags: [],
                  timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: windowNumber, context: nil,
                  eventNumber: 1, clickCount: 0, pressure: 0), let event = wrapped.cgEvent else {
            throw Failure.reason("window_event_unavailable")
        }
        event.setSource(source)
        event.setIntegerValueField(.eventSourceUserData, value: marker)
        guard NSApp == nil, NSEvent(cgEvent: event)?.windowNumber == windowNumber,
              abs(event.location.x - last.x) <= 0.001, abs(event.location.y - last.y) <= 0.001 else {
            throw Failure.reason("window_event_identity_or_pointer_mismatch")
        }
        // Check again after creating the event. Earlier identity and event setup
        // may have taken long enough to make the selected sample stale.
        let readyIdentity = try processIdentity(pid)
        let (readyValues, readyErrors) = monitor.snapshot()
        let readyAt = ProcessInfo.processInfo.systemUptime
        guard samplesUnchanged(readyValues, readyErrors), let readyLast = readyValues.last,
              readyLast.sameState(last), (expectedFront == nil || readyLast.foregroundIdentity == expectedFront),
              isFresh(readyLast, at: readyAt), process.isRunning,
              readyIdentity == identity else {
            throw Failure.reason("sample or child changed during event setup")
        }
        // The single post is to the exact child PID verified above. No global post exists.
        let started = ProcessInfo.processInfo.systemUptime
        guard isFresh(readyLast, at: started) else {
            throw Failure.reason("sample expired immediately before post")
        }
        result["action_attempted"] = true
        postStarted = started
        event.postToPid(pid)
        postFinished = ProcessInfo.processInfo.systemUptime
        try writeNew(["nonce":nonce], to: dir.appendingPathComponent("done.json"))
        let drainDeadline = ProcessInfo.processInfo.systemUptime + 2
        while process.isRunning && ProcessInfo.processInfo.systemUptime < drainDeadline { Thread.sleep(forTimeInterval: 0.02) }
        guard !process.isRunning else { throw Failure.reason("child did not stop after event") }
        let receiptPath = dir.appendingPathComponent("event-1.json")
        let receiptData = try? Data(contentsOf: receiptPath)
        let receipt = receiptData.flatMap { (try? JSONSerialization.jsonObject(with: $0)) as? [String: Any] }
        result["status"] = "experiment_completed"
        result["child_pid"] = pid
        result["marker"] = marker
        result["nonce"] = nonce
        result["child_process_start_unix_seconds"] = identity.startSeconds
        result["child_process_start_microseconds"] = identity.startMicroseconds
        result["child_executable"] = identity.executable
        let finalData = try? Data(contentsOf: dir.appendingPathComponent("final-count.json"))
        let finalCount = finalData.flatMap { (try? JSONSerialization.jsonObject(with: $0)) as? [String: Any] }
        result["child_receipt"] = receipt ?? NSNull()
        result["child_final_count"] = finalCount ?? NSNull()
        let appkitData = try? Data(contentsOf: dir.appendingPathComponent("appkit-1.json"))
        let appkitReceipt = appkitData.flatMap { (try? JSONSerialization.jsonObject(with: $0)) as? [String: Any] }
        result["appkit_receipt"] = appkitReceipt ?? NSNull()
        result["expected_window_number"] = windowNumber
        result["appkit_received_one_tagged_event"] = appkitReceipt.map {
            ($0["nonce"] as? String) == nonce && ($0["child_pid"] as? Int32) == pid &&
            ($0["sender_pid"] as? Int32) == getpid() && ($0["marker"] as? Int64) == marker &&
            ($0["kind"] as? String) == "appkit_mouse_moved" && ($0["count"] as? Int) == 1 &&
            ($0["window_number"] as? Int) == windowNumber
        } == true && (finalCount?["appkit_count"] as? Int) == 1 && (finalCount?["window_hidden"] as? Bool) == true &&
            !FileManager.default.fileExists(atPath: dir.appendingPathComponent("appkit-2.json").path)
        let viewData = try? Data(contentsOf: dir.appendingPathComponent("view-1.json"))
        let viewReceipt = viewData.flatMap { (try? JSONSerialization.jsonObject(with: $0)) as? [String: Any] }
        result["view_receipt"] = viewReceipt ?? NSNull()
        result["view_received_one_tagged_event"] = viewReceipt.map {
            ($0["nonce"] as? String) == nonce && ($0["child_pid"] as? Int32) == pid &&
            ($0["sender_pid"] as? Int32) == getpid() && ($0["marker"] as? Int64) == marker &&
            ($0["kind"] as? String) == "view_mouse_moved" && ($0["count"] as? Int) == 1 &&
            ($0["window_number"] as? Int) == windowNumber
        } == true && (finalCount?["view_count"] as? Int) == 1 && (finalCount?["window_number"] as? Int) == windowNumber &&
            !FileManager.default.fileExists(atPath: dir.appendingPathComponent("view-2.json").path)
        result["child_received_one_tagged_event_in_observation_window"] = process.terminationStatus == 0 &&
            receipt.map { validReceipt($0, child: pid, parent: getpid(), nonce: nonce, marker: marker) } == true &&
            finalCount.map { validFinalCount($0, child: pid, nonce: nonce, marker: marker) } == true &&
            !FileManager.default.fileExists(atPath: dir.appendingPathComponent("event-2.json").path)
        }
    } catch { result["reason"] = String(describing: error) }
    if let process = launched, process.isRunning {
        process.terminate()
        let limit = ProcessInfo.processInfo.systemUptime + 1
        while process.isRunning && ProcessInfo.processInfo.systemUptime < limit { Thread.sleep(forTimeInterval: 0.02) }
    }
    let stopped = launched == nil || !(launched?.isRunning ?? true)
    result["child_stopped"] = stopped
    if stopped, launched != nil, result["reason"] as? String == "child tap readiness unavailable" {
        let errorURL = dir.appendingPathComponent("child-stderr.json")
        if let attributes = try? FileManager.default.attributesOfItem(atPath: errorURL.path),
           let size = attributes[.size] as? NSNumber, size.intValue <= 4096,
           let data = try? Data(contentsOf: errorURL) {
            result["child_failure_category"] = childFailureCategory(data)
        } else { result["child_failure_category"] = "other_or_unreadable" }
    }
    Thread.sleep(forTimeInterval: 0.15)
    let (values, errors, failureKinds) = monitor.detailedSnapshot()
    let covered = coversPost(values, started: postStarted, finished: postFinished)
    result["post_sampling_coverage"] = covered
    result["sampled_state_unchanged"] = samplesUnchanged(values, errors) && covered
    if observeOnly { result["lifecycle_sampled_unchanged"] = samplesUnchanged(values, errors) }
    result["sample_count"] = values.count
    result["foreground_identity"] = values.first.map { $0.foregroundIdentity.json as Any } ?? NSNull()
    result["expected_foreground_matched"] = expectedFront != nil && samplesUnchanged(values, errors) && values.first?.foregroundIdentity == expectedFront
    result["sample_errors"] = errors
    result["sample_error_categories"] = failureKinds
    result["sample_diagnostics"] = sampleDiagnostics(values, errors)
    let preserve = retainTrialEvidence(actionAttempted: result["action_attempted"] as? Bool == true,
        status: result["status"] as? String,
        tapReceipt: result["child_received_one_tagged_event_in_observation_window"] as? Bool == true,
        appkitReceipt: result["appkit_received_one_tagged_event"] as? Bool == true,
        viewReceipt: result["view_received_one_tagged_event"] as? Bool == true,
        stable: samplesUnchanged(values, errors), covered: covered, stopped: stopped)
    if preserve {
        result["cleanup_status"] = stopped ? "preserved_for_review" : "not_attempted_child_running"
        result["cleanup_directory"] = dir.path
        let names = ["ready.json", "done.json", "event-1.json", "event-2.json", "appkit-1.json", "appkit-2.json", "view-1.json", "view-2.json", "final-count.json"]
        result["retained_files"] = names.filter { FileManager.default.fileExists(atPath: dir.appendingPathComponent($0).path) }
    } else {
        let cleanupResult = cleanup(stopped: stopped) { try FileManager.default.removeItem(at: dir) }
        result["cleanup_status"] = cleanupResult.status
        if let error = cleanupResult.error {
            result["cleanup_error"] = error
            result["cleanup_directory"] = dir.path
        }
    }
    if result["status"] as? String == "experiment_completed" && !completedSuccessfully(result) {
        result["status"] = "error"
    }
    return result
}

@main
private struct Main {
    static func main() {
        do {
            let args = Array(CommandLine.arguments.dropFirst())
            if args.first == "--child" { try child(args); return }
            let result: [String: Any]
            if args == ["--self-test"] { try selfTest(); result = ["status":"self_test_passed", "live_ui_access":false] }
            else if args == ["--sample-only"] {
                let monitor = Monitor(); monitor.start()
                Thread.sleep(forTimeInterval: 0.35)
                let (samples, failures, categories) = monitor.detailedSnapshot()
                monitor.stop()
                let unchanged = samplesUnchanged(samples, failures)
                result = ["status":unchanged ? "sampled" : "unknown", "input_posted":false, "action_allowed":false,
                    "sample_count":samples.count, "sample_errors":failures, "sample_error_categories":categories,
                    "sampled_state_unchanged":unchanged, "sample_diagnostics":sampleDiagnostics(samples, failures), "nsapp_present":NSApp != nil,
                    "note":"Read-only sampled baseline; no claim of input isolation, event delivery, or uninterrupted stability."]
                let data = try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys])
                FileHandle.standardOutput.write(data); FileHandle.standardOutput.write(Data([10]))
                exit(unchanged ? 0 : 3)
            }
            else if args == ["--run-experiment"] || args == ["--observe-lifecycle"] {
                var expected: ForegroundIdentity?
                if let raw = ProcessInfo.processInfo.environment["CU_EXPECTED_FOREGROUND_IDENTITY"] {
                    guard raw.utf8.count <= 512, let data = raw.data(using: .utf8),
                          let value = try? JSONSerialization.jsonObject(with: data),
                          let decoded = ForegroundIdentity.decode(value) else { throw Failure.reason("invalid expected foreground identity") }
                    expected = decoded
                }
                result = try runExperiment(expectedFront: expected, observeOnly: args == ["--observe-lifecycle"])
            }
            else { throw Failure.reason("use --self-test, --sample-only, --observe-lifecycle, or explicitly --run-experiment; external target IDs are not accepted") }
            let data = try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys])
            FileHandle.standardOutput.write(data); FileHandle.standardOutput.write(Data([10]))
            let lifecycleOK = result["status"] as? String == "lifecycle_observed" && result["action_attempted"] as? Bool == false &&
                result["child_stopped"] as? Bool == true && result["cleanup_status"] as? String == "completed" &&
                result["lifecycle_sampled_unchanged"] as? Bool == true
            if result["status"] as? String != "self_test_passed" && !completedSuccessfully(result) && !lifecycleOK { exit(3) }
        } catch {
            let data = try! JSONSerialization.data(withJSONObject: ["status":"stopped", "reason":String(describing: error), "general_input_authorized":false])
            FileHandle.standardError.write(data); FileHandle.standardError.write(Data([10])); exit(1)
        }
    }
}
