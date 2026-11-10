// Opt-in local AXPress experiment. Never targets a caller-supplied process.
import AppKit
import ApplicationServices
import Foundation

private enum Failure: Error, CustomStringConvertible {
    case reason(String)
    var description: String { if case .reason(let s) = self { return s }; return "unknown" }
}

private struct Sample: Equatable {
    let uptime: Double
    let foregroundPID: Int32
    let focusedPID: Int32
    let focusedHash: UInt
    let x: Double
    let y: Double
    func sameState(_ other: Sample) -> Bool {
        foregroundPID == other.foregroundPID && focusedPID == other.focusedPID &&
        focusedHash == other.focusedHash && x == other.x && y == other.y
    }
    var json: [String: Any] {
        ["uptime_seconds": uptime, "foreground_pid": foregroundPID, "focused_pid": focusedPID,
         "focused_element_hash": focusedHash, "cursor_x": x, "cursor_y": y]
    }
}

private func sample() throws -> Sample {
    guard let front = NSWorkspace.shared.frontmostApplication, let event = CGEvent(source: nil) else {
        throw Failure.reason("foreground or cursor unavailable")
    }
    let system = AXUIElementCreateSystemWide()
    _ = AXUIElementSetMessagingTimeout(system, 0.2)
    var raw: CFTypeRef?
    guard AXUIElementCopyAttributeValue(system, kAXFocusedUIElementAttribute as CFString, &raw) == .success,
          let value = raw, CFGetTypeID(value) == AXUIElementGetTypeID() else {
        throw Failure.reason("focused element unavailable")
    }
    let focus = unsafeBitCast(value, to: AXUIElement.self)
    var pid: pid_t = 0
    guard AXUIElementGetPid(focus, &pid) == .success, pid > 0,
          event.location.x.isFinite, event.location.y.isFinite else {
        throw Failure.reason("focused element or cursor identity unavailable")
    }
    return Sample(uptime: ProcessInfo.processInfo.systemUptime, foregroundPID: front.processIdentifier,
                  focusedPID: pid, focusedHash: CFHash(focus), x: event.location.x, y: event.location.y)
}

private final class Monitor {
    private let lock = NSLock()
    private var records: [Sample] = []
    private var failures: [String] = []
    private let timer = DispatchSource.makeTimerSource(queue: DispatchQueue(label: "ax-fixture-monitor"))
    func start() {
        timer.schedule(deadline: .now(), repeating: .milliseconds(20))
        timer.setEventHandler { [weak self] in
            guard let self else { return }
            do { let value = try sample(); self.lock.lock(); self.records.append(value); self.lock.unlock() }
            catch { self.lock.lock(); self.failures.append(String(describing: error)); self.lock.unlock() }
        }
        timer.resume()
    }
    func snapshot() -> ([Sample], [String]) {
        lock.lock(); defer { lock.unlock() }; return (records, failures)
    }
    func stop() { timer.cancel() }
}

private func unchanged(_ samples: [Sample], _ failures: [String]) -> Bool {
    guard let first = samples.first, samples.count >= 3, failures.isEmpty else { return false }
    return samples.allSatisfy { $0.sameState(first) }
}

private func readAttr(_ el: AXUIElement, _ name: CFString) -> CFTypeRef? {
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(el, name, &value) == .success else { return nil }
    return value
}
private func stringAttr(_ el: AXUIElement, _ name: CFString) -> String? {
    guard let value = readAttr(el, name), CFGetTypeID(value) == CFStringGetTypeID() else { return nil }
    return value as? String
}
private func children(_ el: AXUIElement, _ name: CFString) -> [AXUIElement]? {
    guard let value = readAttr(el, name), CFGetTypeID(value) == CFArrayGetTypeID(),
          let items = value as? [AnyObject], items.count <= 32 else { return nil }
    var result: [AXUIElement] = []
    for item in items {
        guard CFGetTypeID(item) == AXUIElementGetTypeID() else { return nil }
        result.append(unsafeBitCast(item, to: AXUIElement.self))
    }
    return result
}
private func ownerMatches(_ el: AXUIElement, _ pid: pid_t) -> Bool {
    var owner: pid_t = 0
    return AXUIElementGetPid(el, &owner) == .success && owner == pid
}
private func findButton(pid: pid_t, nonce: String) throws -> AXUIElement {
    let root = AXUIElementCreateApplication(pid)
    _ = AXUIElementSetMessagingTimeout(root, 0.2)
    guard ownerMatches(root, pid), let windows = children(root, kAXWindowsAttribute as CFString),
          windows.count == 1 else { throw Failure.reason("fixture window not unique") }
    let window = windows[0]
    _ = AXUIElementSetMessagingTimeout(window, 0.2)
    guard ownerMatches(window, pid),
          stringAttr(window, kAXTitleAttribute as CFString) == "AX Press Fixture \(nonce)",
          let nodes = children(window, kAXChildrenAttribute as CFString) else {
        throw Failure.reason("fixture window identity mismatch")
    }
    let buttons = nodes.filter { el in
        _ = AXUIElementSetMessagingTimeout(el, 0.2)
        return ownerMatches(el, pid) && stringAttr(el, kAXRoleAttribute as CFString) == "AXButton" &&
            stringAttr(el, kAXTitleAttribute as CFString) == "Fixture Button \(nonce)" &&
            stringAttr(el, "AXIdentifier" as CFString) == "ax-press-fixture-\(nonce)"
    }
    guard buttons.count == 1 else { throw Failure.reason("fixture button not unique or not direct child") }
    var names: CFArray?
    guard AXUIElementCopyActionNames(buttons[0], &names) == .success,
          let actions = names as? [String], actions.contains(kAXPressAction as String) else {
        throw Failure.reason("fixture does not expose AXPress")
    }
    return buttons[0]
}

private func testData() throws {
    let baseline = Sample(uptime: 1, foregroundPID: 2, focusedPID: 2, focusedHash: 3, x: 4, y: 5)
    let equal = Sample(uptime: 2, foregroundPID: 2, focusedPID: 2, focusedHash: 3, x: 4, y: 5)
    let changed = Sample(uptime: 2, foregroundPID: 2, focusedPID: 2, focusedHash: 3, x: 4.1, y: 5)
    guard !unchanged([], []), !unchanged([baseline, equal], []),
          unchanged([baseline, equal, equal], []), !unchanged([baseline, equal, changed], []),
          !unchanged([baseline, equal, equal], ["missing"]) else { throw Failure.reason("comparison self-test failed") }
}

private func runFixture() throws -> [String: Any] {
    guard AXIsProcessTrusted() else { throw Failure.reason("accessibility permission unavailable; no prompt requested") }
    let binary = URL(fileURLWithPath: CommandLine.arguments[0]).standardizedFileURL
    let fixture = binary.deletingLastPathComponent().appendingPathComponent("AXPressFixture.app/Contents/MacOS/AXPressFixture")
        .resolvingSymlinksInPath()
    guard FileManager.default.isExecutableFile(atPath: fixture.path) else { throw Failure.reason("fixture executable missing") }
    let nonce = UUID().uuidString
    let directory = URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true).appendingPathComponent("ax-press-\(nonce)")
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false,
                                            attributes: [.posixPermissions: 0o700])
    let ready = directory.appendingPathComponent("ready")
    let firstPress = directory.appendingPathComponent("count-1")
    let secondPress = directory.appendingPathComponent("count-2")
    let monitor = Monitor()
    monitor.start()
    defer { monitor.stop() }
    var result: [String: Any] = ["status": "stopped", "action_attempted": false,
        "general_input_authorized": false,
        "note": "Sampling can miss brief cursor or focus changes. Fixture results do not prove other apps or game input are safe."]
    var launched: Process?
    do {
        let baselineDeadline = ProcessInfo.processInfo.systemUptime + 1.5
        var (before, errors) = monitor.snapshot()
        while before.count < 3 && errors.isEmpty && ProcessInfo.processInfo.systemUptime < baselineDeadline {
            Thread.sleep(forTimeInterval: 0.05)
            (before, errors) = monitor.snapshot()
        }
        guard unchanged(before, errors) else { throw Failure.reason("cannot establish stable baseline") }
        let child = Process()
        child.executableURL = fixture
        child.arguments = ["--child-fixture", String(getpid()), nonce]
        var environment = ProcessInfo.processInfo.environment
        environment["AX_FIXTURE_PRIVATE_DIR"] = directory.path
        child.environment = environment
        child.standardOutput = FileHandle.nullDevice
        child.standardError = FileHandle.nullDevice
        try child.run()
        launched = child
        let pid = child.processIdentifier
        result["fixture_pid"] = pid
        let deadline = ProcessInfo.processInfo.systemUptime + 6
        while child.isRunning && !FileManager.default.fileExists(atPath: ready.path) && ProcessInfo.processInfo.systemUptime < deadline {
            Thread.sleep(forTimeInterval: 0.05)
        }
        guard child.isRunning, FileManager.default.fileExists(atPath: ready.path),
              let app = NSRunningApplication(processIdentifier: pid), let started = app.launchDate,
              app.executableURL?.resolvingSymlinksInPath().path == fixture.path,
              app.bundleIdentifier == "org.local.rust-computer-use.ax-press-fixture" else {
            throw Failure.reason("child did not establish fixture process identity")
        }
        let button = try findButton(pid: pid, nonce: nonce)
        // Keep sampling after the window appears before attempting the action.
        Thread.sleep(forTimeInterval: 0.15)
        (before, errors) = monitor.snapshot()
        guard unchanged(before, errors), child.isRunning,
              let check = NSRunningApplication(processIdentifier: pid), check.launchDate == started,
              check.executableURL?.resolvingSymlinksInPath().path == fixture.path,
              ownerMatches(button, pid), !FileManager.default.fileExists(atPath: firstPress.path),
              !FileManager.default.fileExists(atPath: secondPress.path),
              ProcessInfo.processInfo.systemUptime < deadline else {
            throw Failure.reason("fixture changed, baseline changed, or deadline passed before press")
        }
        // This is the sole input operation. The PID is a child launched above, never a CLI argument.
        result["action_attempted"] = true
        let action = AXUIElementPerformAction(button, kAXPressAction as CFString)
        Thread.sleep(forTimeInterval: 0.35)
        result["status"] = "experiment_completed"
        result["ax_error"] = action.rawValue
        result["fixture_count_is_one"] = (try? String(contentsOf: firstPress, encoding: .utf8)) == "pressed" &&
            !FileManager.default.fileExists(atPath: secondPress.path)
    } catch { result["reason"] = String(describing: error) }
    if let child = launched, child.isRunning {
        child.terminate()
        let cleanupDeadline = ProcessInfo.processInfo.systemUptime + 1
        while child.isRunning && ProcessInfo.processInfo.systemUptime < cleanupDeadline { Thread.sleep(forTimeInterval: 0.02) }
    }
    let stopped = launched == nil || !(launched?.isRunning ?? true)
    if stopped { try? FileManager.default.removeItem(at: directory) }
    result["fixture_stopped"] = stopped
    Thread.sleep(forTimeInterval: 0.15)
    let (records, failures) = monitor.snapshot()
    result["sampled_state_unchanged"] = unchanged(records, failures)
    result["sample_count"] = records.count
    result["samples"] = records.map(\.json)
    result["sampling_errors"] = failures
    return result
}

@main
struct Main {
    static func main() {
        do {
            let value: [String: Any]
            switch Array(CommandLine.arguments.dropFirst()) {
            case ["--self-test"]:
                try testData(); value = ["status": "self_test_passed", "live_ui_access": false]
            case ["--run-fixture"]: throw Failure.reason("legacy visible AX fixture disabled: focus isolation and uncertain receipt retention are unverified")
            default: throw Failure.reason("use --self-test or explicitly --run-fixture")
            }
            let data = try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
            FileHandle.standardOutput.write(data); FileHandle.standardOutput.write(Data([10]))
            if value["status"] as? String == "stopped" { exit(3) }
            if value["status"] as? String == "experiment_completed" &&
                (value["ax_error"] as? Int32 != 0 || value["fixture_count_is_one"] as? Bool != true ||
                 value["sampled_state_unchanged"] as? Bool != true || value["fixture_stopped"] as? Bool != true) {
                exit(3)
            }
        } catch {
            let data = try! JSONSerialization.data(withJSONObject: ["status": "stopped", "reason": String(describing: error), "general_input_authorized": false])
            FileHandle.standardError.write(data); FileHandle.standardError.write(Data([10])); exit(1)
        }
    }
}
