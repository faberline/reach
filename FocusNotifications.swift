// Read-only AX notification sample or bounded watch. No event posting or UI changes.
import AppKit
import ApplicationServices
import Foundation
import Darwin

final class Counts {
    var element = 0; var window = 0; var deactivated = 0; var watch = false
    var focusChanged: Bool { element > 0 || window > 0 || deactivated > 0 }
    func record(_ name: String) -> String? {
        guard let kind = notificationKind(name) else { return nil }
        switch kind {
        case "focused_element": element += 1
        case "focused_window": window += 1
        case "deactivated": deactivated += 1
        default: return nil
        }
        return kind
    }
}
private func notificationKind(_ name: String) -> String? {
    switch name {
    case kAXFocusedUIElementChangedNotification: return "focused_element"
    case kAXFocusedWindowChangedNotification: return "focused_window"
    case kAXApplicationDeactivatedNotification: return "deactivated"
    default: return nil
    }
}
private let callback: AXObserverCallback = { _, _, name, refcon in
    guard let refcon else { return }
    let counts = Unmanaged<Counts>.fromOpaque(refcon).takeUnretainedValue()
    guard let kind = counts.record(name as String) else { return }
    if counts.watch {
        emit(["status":"focus_changed", "kind":kind, "action_allowed":false, "input_posted":false])
    }
}
private func emit(_ value: [String: Any]) {
    let bytes = try! JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
    FileHandle.standardOutput.write(bytes); FileHandle.standardOutput.write(Data([10]))
}
private func stopped(_ reason: String) -> Never {
    emit(["status":"unknown", "action_allowed":false, "input_posted":false, "reason":reason]); exit(1)
}
// The watch control accepts only the fixed command. It never returns input text.
struct StopControl {
    private var buffer: [UInt8] = []
    mutating func feed(_ bytes: [UInt8]) -> String? {
        buffer.append(contentsOf: bytes)
        let expected = Array("stop\n".utf8)
        guard buffer.count <= expected.count, Array(expected.prefix(buffer.count)) == buffer else { return "invalid_input" }
        return buffer.count == expected.count ? "requested" : nil
    }
}
private func selfTest() {
    var a = StopControl(), b = StopControl(), c = StopControl()
    let counter = Counts()
    guard !counter.focusChanged, counter.record("unrecognized") == nil, !counter.focusChanged,
          counter.record(kAXApplicationDeactivatedNotification) == "deactivated", counter.focusChanged,
          counter.deactivated == 1 else { stopped("self_test_failed") }
    guard notificationKind(kAXFocusedUIElementChangedNotification) == "focused_element",
          notificationKind(kAXFocusedWindowChangedNotification) == "focused_window",
          notificationKind(kAXApplicationDeactivatedNotification) == "deactivated",
          notificationKind("unrecognized") == nil,
          a.feed(Array("st".utf8)) == nil, a.feed(Array("op\n".utf8)) == "requested",
          b.feed(Array("secret".utf8)) == "invalid_input", c.feed(Array("stop!".utf8)) == "invalid_input" else { stopped("self_test_failed") }
    emit(["status":"self_test_passed", "live_ui_access":false, "input_posted":false])
}
// The live test child accepts commands only from another instance of this
// executable and posts one AX notification to itself, never to an external PID.
struct ForegroundIdentity: Equatable {
    let pid: pid_t
    let startedSeconds: Int64
    let startedMicroseconds: Int32
    var json: [String: Any] { ["pid":pid, "started_seconds":startedSeconds, "started_microseconds":startedMicroseconds] }
}
private func foregroundIdentity() -> ForegroundIdentity? {
    guard let app = NSWorkspace.shared.frontmostApplication, app.processIdentifier > 0 else { return nil }
    let pid = app.processIdentifier
    var mib = [CTL_KERN, KERN_PROC, KERN_PROC_PID, pid]
    var info = kinfo_proc()
    var size = MemoryLayout<kinfo_proc>.size
    guard sysctl(&mib, 4, &info, &size, nil, 0) == 0, size == MemoryLayout<kinfo_proc>.size,
          info.kp_proc.p_pid == pid, info.kp_proc.p_starttime.tv_sec > 0,
          (0..<1_000_000).contains(info.kp_proc.p_starttime.tv_usec),
          NSWorkspace.shared.frontmostApplication?.processIdentifier == pid else { return nil }
    return ForegroundIdentity(pid: pid, startedSeconds: Int64(info.kp_proc.p_starttime.tv_sec),
                              startedMicroseconds: Int32(info.kp_proc.p_starttime.tv_usec))
}
private func executablePath(_ pid: pid_t) -> String? {
    var path = [CChar](repeating: 0, count: 4 * Int(MAXPATHLEN))
    guard proc_pidpath(pid, &path, UInt32(path.count)) > 0 else { return nil }
    return URL(fileURLWithPath: String(cString: path)).resolvingSymlinksInPath().path
}
private func runTestChild(_ args: [String]) -> Never {
    guard args.count == 3, let expected = pid_t(args[2]), expected > 0, getppid() == expected,
          let own = executablePath(getpid()), own == executablePath(expected) else { stopped("invalid_test_parent") }
    let app = NSApplication.shared
    if app.activationPolicy() != .prohibited { _ = app.setActivationPolicy(.prohibited) }
    guard app.activationPolicy() == .prohibited, app.windows.isEmpty else { stopped("test_child_not_headless") }
    DispatchQueue.main.async { emit(["status":"test_child_ready"]) }
    DispatchQueue.global().async {
        guard readLine() == "notify", getppid() == expected else { DispatchQueue.main.async { app.terminate(nil) }; return }
        DispatchQueue.main.async {
            NSAccessibility.post(element: app, notification: .focusedUIElementChanged)
            DispatchQueue.main.asyncAfter(deadline: .now() + 1) { app.terminate(nil) }
        }
    }
    DispatchQueue.main.asyncAfter(deadline: .now() + 8) { app.terminate(nil) }
    app.run()
    exit(0)
}
if CommandLine.arguments.count >= 2 && CommandLine.arguments[1] == "--internal-test-child" {
    runTestChild(CommandLine.arguments)
}
guard CommandLine.arguments.count == 2 else { stopped("invalid_arguments") }
let mode = CommandLine.arguments[1]
if mode == "--self-test" { selfTest(); exit(0) }
guard mode == "--sample" || mode == "--watch" || mode == "--test-own-notification" else { stopped("invalid_arguments") }
let testOwn = mode == "--test-own-notification"
let watching = mode == "--watch" || testOwn
if mode == "--watch" && isatty(STDIN_FILENO) != 0 { stopped("watch_requires_pipe") }
guard AXIsProcessTrusted() else { stopped("accessibility_unavailable") }
guard let initialIdentity = foregroundIdentity() else { stopped("foreground_identity_unavailable") }
let frontPID = initialIdentity.pid
var testChild: Process?
var testChildInput: Pipe?
if testOwn {
    guard let own = executablePath(getpid()) else { stopped("own_executable_unavailable") }
    let child = Process(), input = Pipe(), output = Pipe()
    child.executableURL = URL(fileURLWithPath: own)
    child.arguments = ["--internal-test-child", String(getpid())]
    child.standardInput = input; child.standardOutput = output; child.standardError = FileHandle.nullDevice
    do { try child.run() } catch { stopped("test_child_launch_failed") }
    testChild = child; testChildInput = input
    // The child writes one short, fixed ready line after its AppKit loop starts.
    var line = Data()
    while line.count < 128 {
        guard let chunk = try? output.fileHandleForReading.read(upToCount: 1), !chunk.isEmpty else { break }
        if chunk[0] == 10 { break }; line.append(chunk)
    }
    guard child.isRunning, let ready = (try? JSONSerialization.jsonObject(with: line)) as? [String: Any],
          ready["status"] as? String == "test_child_ready", foregroundIdentity() == initialIdentity else {
        if child.isRunning { child.terminate() }; child.waitUntilExit(); stopped("test_child_not_ready")
    }
}
let pid = testChild?.processIdentifier ?? frontPID
let target = AXUIElementCreateApplication(pid)
var observer: AXObserver?
guard AXObserverCreate(pid, callback, &observer) == .success, let observer else {
    if let child = testChild { if child.isRunning { child.terminate() }; child.waitUntilExit() }
    stopped("observer_unavailable")
}
let counts = Counts()
counts.watch = watching
let ptr = Unmanaged.passUnretained(counts).toOpaque()
let names = [kAXFocusedUIElementChangedNotification, kAXFocusedWindowChangedNotification, kAXApplicationDeactivatedNotification]
let codes = names.map { AXObserverAddNotification(observer, target, $0 as CFString, ptr) }
let source = AXObserverGetRunLoopSource(observer)
CFRunLoopAddSource(CFRunLoopGetCurrent(), source, .defaultMode)
let allRegistered = codes.allSatisfy { $0 == .success }
let start = ProcessInfo.processInfo.systemUptime
var termination: String?
var control = StopControl()
if watching {
    guard (testOwn ? codes[0] == .success : allRegistered), foregroundIdentity() == initialIdentity else {
        for (i, name) in names.enumerated() where codes[i] == .success {
            _ = AXObserverRemoveNotification(observer, target, name as CFString)
        }
        CFRunLoopRemoveSource(CFRunLoopGetCurrent(), source, .defaultMode)
        if let child = testChild { if child.isRunning { child.terminate() }; child.waitUntilExit() }
        stopped("watch_not_ready")
    }
    emit(["status":"ready", "action_allowed":false, "input_posted":false,
          "scope":testOwn ? "own_child_test" : "frontmost_app", "foreground_identity":initialIdentity.json,
          "registration":["focused_element":codes[0] == .success, "focused_window":codes[1] == .success, "deactivated":codes[2] == .success],
          "max_seconds":testOwn ? 3 : 30])
    if testOwn {
        guard let input = testChildInput, (try? input.fileHandleForWriting.write(contentsOf: Data("notify\n".utf8))) != nil else {
            if let child = testChild { if child.isRunning { child.terminate() }; child.waitUntilExit() }
            stopped("test_child_command_failed")
        }
    }
}
while termination == nil && ProcessInfo.processInfo.systemUptime - start < (testOwn ? 3.0 : (mode == "--watch" ? 30.0 : 1.0)) {
    _ = CFRunLoopRunInMode(.defaultMode, 0.05, false)
    if watching {
        if counts.focusChanged { termination = "focus_changed"; break }
        if testOwn { continue }
        var descriptor = pollfd(fd: STDIN_FILENO, events: Int16(POLLIN), revents: 0)
        let pollResult = poll(&descriptor, 1, 0)
        if pollResult < 0 || descriptor.revents & Int16(POLLERR | POLLNVAL) != 0 {
            termination = "input_error"; break
        }
        if pollResult > 0 && descriptor.revents & Int16(POLLIN | POLLHUP) != 0 {
            var bytes = [UInt8](repeating: 0, count: 6)
            let n = read(STDIN_FILENO, &bytes, bytes.count)
            if n == 0 { termination = "end_of_input" }
            else if n < 0 { termination = "input_error" }
            else { termination = control.feed(Array(bytes.prefix(n))) }
        }
    }
}
if watching && termination == nil { termination = "timeout" }
if mode == "--watch" && termination == "requested" {
    // Drain already queued notifications before confirming normal stop.
    let drainUntil = ProcessInfo.processInfo.systemUptime + 0.1
    while ProcessInfo.processInfo.systemUptime < drainUntil && !counts.focusChanged {
        _ = CFRunLoopRunInMode(.defaultMode, 0.02, false)
    }
    if counts.focusChanged { termination = "focus_changed" }
}
let end = ProcessInfo.processInfo.systemUptime
let endIdentity = foregroundIdentity()
let frontSame = endIdentity == initialIdentity
if mode == "--watch" && termination == "requested" && !frontSame { termination = "foreground_changed" }
for (i, name) in names.enumerated() where codes[i] == .success {
    _ = AXObserverRemoveNotification(observer, target, name as CFString)
}
CFRunLoopRemoveSource(CFRunLoopGetCurrent(), source, .defaultMode)
var output: [String: Any] = ["status":allRegistered ? "sampled" : "sampled_partial", "action_allowed":false, "input_posted":false,
      "registration":["focused_element":codes[0] == .success, "focused_window":codes[1] == .success, "deactivated":codes[2] == .success],
      "counts":["focused_element":counts.element, "focused_window":counts.window, "deactivated":counts.deactivated],
      "frontmost_same_at_end":frontSame, "foreground_identity":initialIdentity.json,
      "foreground_identity_at_end":endIdentity.map { $0.json as Any } ?? NSNull(), "duration_seconds":end-start,
      "note": testOwn
        ? "Only the program's own headless child is observed. The child posts one AX test notification to itself; no input isolation is proven."
        : "Only the initially foreground app is observed. Notifications may be unsupported, delayed or missing; no input isolation is proven."]
if let termination { output["termination"] = termination }
if let child = testChild {
    if child.isRunning { child.terminate() }
    child.waitUntilExit()
    output["test_child_stopped"] = !child.isRunning
    output["scope"] = "own_child_test"
}
emit(output)
withExtendedLifetime(counts) {}
if watching && termination != "requested" { exit(3) }
