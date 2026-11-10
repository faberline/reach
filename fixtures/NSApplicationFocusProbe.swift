// Explicit opt-in diagnostic. No explicit activation, window creation, or event posting.
import AppKit
import ApplicationServices
import Foundation

private func focusedStatus() -> (code: Int32, validElement: Bool) {
    let system = AXUIElementCreateSystemWide()
    _ = AXUIElementSetMessagingTimeout(system, 0.2)
    var value: CFTypeRef?
    let code = AXUIElementCopyAttributeValue(system, kAXFocusedApplicationAttribute as CFString, &value)
    return (code.rawValue, code == .success && value.map { CFGetTypeID($0) == AXUIElementGetTypeID() } == true)
}

private func encode(_ value: [String: Any]) {
    let data = try! JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data([10]))
}

private struct Sample {
    let started: Double
    let ended: Double
    let front: pid_t
    let cursor: CGPoint
    let windowOrder: [UInt32]
}

private func stable(_ samples: [Sample], errors: Int) -> Bool {
    guard errors == 0, samples.count >= 3, let first = samples.first else { return false }
    var previous: Double?
    for sample in samples {
        guard sample.front == first.front, sample.cursor == first.cursor, sample.windowOrder == first.windowOrder,
              sample.started.isFinite, sample.ended.isFinite, sample.ended >= sample.started,
              sample.ended - sample.started <= 0.25 else { return false }
        if let previous, sample.started < previous || sample.started - previous > 0.25 { return false }
        previous = sample.ended
    }
    return true
}

private final class Monitor {
    private let lock = NSLock()
    private var samples: [Sample] = []
    private var errors = 0
    private let timer = DispatchSource.makeTimerSource(queue: DispatchQueue(label: "focus-probe-monitor"))
    func start() {
        timer.schedule(deadline: .now(), repeating: .milliseconds(20))
        timer.setEventHandler { [weak self] in
            guard let self else { return }
            let start = ProcessInfo.processInfo.systemUptime
            let front = NSWorkspace.shared.frontmostApplication?.processIdentifier
            let cursor = CGEvent(source: nil)?.location
            let windows = CGWindowListCopyWindowInfo(.optionOnScreenOnly, kCGNullWindowID) as? [[String: Any]]
            let order = windows?.compactMap { ($0[kCGWindowNumber as String] as? NSNumber)?.uint32Value }
            let end = ProcessInfo.processInfo.systemUptime
            self.lock.lock(); defer { self.lock.unlock() }
            guard let front, front > 0, let cursor, cursor.x.isFinite, cursor.y.isFinite,
                  let windows, let order, !order.isEmpty, order.count == windows.count,
                  order.count <= 4096, !order.contains(0), Set(order).count == order.count else { self.errors += 1; return }
            self.samples.append(Sample(started: start, ended: end, front: front, cursor: cursor, windowOrder: order))
        }
        timer.resume()
    }
    func snapshot() -> ([Sample], Int) { lock.lock(); defer { lock.unlock() }; return (samples, errors) }
    func stop() { timer.cancel() }
}

@main
private struct Main {
    static func main() {
        let args = Array(CommandLine.arguments.dropFirst())
        if args == ["--self-test"] {
            let a = Sample(started: 1, ended: 1.01, front: 1, cursor: .zero, windowOrder: [10,11])
            let b = Sample(started: 1.02, ended: 1.03, front: 1, cursor: .zero, windowOrder: [10,11])
            let c = Sample(started: 1.04, ended: 1.05, front: 1, cursor: .zero, windowOrder: [10,11])
            let changed = Sample(started: 1.04, ended: 1.05, front: 2, cursor: .zero, windowOrder: [10,11])
            let reordered = Sample(started: 1.04, ended: 1.05, front: 1, cursor: .zero, windowOrder: [11,10])
            guard stable([a,b,c], errors: 0), !stable([a,b], errors: 0),
                  !stable([a,b,changed], errors: 0), !stable([a,b,reordered], errors: 0),
                  !stable([a,b,c], errors: 1) else { exit(1) }
            encode(["status":"self_test_passed","live_ui_access":false,"input_posted":false])
            return
        }
        guard args == ["--run-probe"] else {
            encode(["status":"stopped","reason":"explicit --run-probe required","input_posted":false,"action_allowed":false])
            exit(2)
        }
        let monitor = Monitor(); monitor.start(); defer { monitor.stop() }
        Thread.sleep(forTimeInterval: 0.2)
        let (baseline, baselineErrors) = monitor.snapshot()
        guard stable(baseline, errors: baselineErrors) else {
            encode(["status":"stopped","reason":"unstable baseline","input_posted":false,"action_allowed":false])
            return
        }
        let beforeStatus = focusedStatus()
        let applicationAlreadyInitialized = NSApp != nil
        let initializationStarted = ProcessInfo.processInfo.systemUptime
        let (ready, readyErrors) = monitor.snapshot()
        guard stable(ready, errors: readyErrors), let last = ready.last,
              initializationStarted >= last.ended, initializationStarted - last.ended <= 0.25 else {
            encode(["status":"stopped","reason":"baseline changed or expired","input_posted":false,"action_allowed":false])
            return
        }
        let app = NSApplication.shared
        let setPolicySucceeded = app.setActivationPolicy(.prohibited)
        let actualPolicy = app.activationPolicy()
        let prohibited = setPolicySucceeded && actualPolicy == .prohibited
        let initializationEnded = ProcessInfo.processInfo.systemUptime
        let afterStatus = focusedStatus()
        Thread.sleep(forTimeInterval: 0.2)
        let (samples, errors) = monitor.snapshot()
        let pre = samples.filter { $0.ended <= initializationStarted }
        let post = samples.filter { $0.started >= initializationEnded }
        let covered = pre.count >= 3 && post.count >= 3 &&
            pre.last.map { initializationStarted - $0.ended <= 0.25 } == true &&
            post.first.map { $0.started - initializationEnded <= 0.25 } == true
        encode(["status":"observed","before_focus_status":beforeStatus.code,"before_focus_valid":beforeStatus.validElement,
                "after_focus_status":afterStatus.code,"after_focus_valid":afterStatus.validElement,
                "activation_policy_prohibited":prohibited,
                "application_already_initialized_before_probe":applicationAlreadyInitialized,
                "set_activation_policy_succeeded":setPolicySucceeded,"actual_activation_policy_raw":actualPolicy.rawValue,
                "sampling_covers_initialization":covered,"sampled_state_unchanged":covered && stable(samples, errors: errors),
                "sample_count":samples.count,"sample_errors":errors,
                "input_posted":false,"action_allowed":false,
                "note":"Before and after samples can miss brief changes; no input isolation is proven."])
    }
}
