// The system input backend for ReachAgent (plan I-2, faberline/reach#27).
//
// This is the ONLY source file that may name an event delivery API or the post-event
// permission request; tests/foreground_input_static.rs scans every Swift file that the
// build scripts compile and fails if one appears anywhere else.
//
// It is compiled in, but nothing can reach it unless all of these hold at start-up:
//   - ReachAgent runs `--serve` with `--enable-input` and its arming options,
//   - `--test-mode` is absent,
//   - the environment has no non-empty `CI` variable.
// `SystemInputLaunch.authorize` checks the three conditions. Its initializer is fileprivate,
// so no other file can make a `SystemInputLaunch`, and `makeSystemInputBackend` and
// `requestPostEventAccess` both require one. `--self-test` asserts that `authorize`
// returns nil in test mode, under CI and without input.
//
// The live facts are fail-closed today: focus, HID idle, secure input, input source and
// occlusion are not sampled yet (plan I-3), so the gate refuses every request on this
// backend at G7, G8 or G9 before any event could be posted.
import AppKit
import CoreGraphics
import Foundation

/// Proof that start-up allowed the system input backend.
struct SystemInputLaunch {
    fileprivate init() {}

    static func authorize(enableInput: Bool, testMode: Bool, environment: [String: String]) -> SystemInputLaunch? {
        guard enableInput, !testMode else { return nil }
        if let ci = environment["CI"], !ci.isEmpty { return nil }
        return SystemInputLaunch()
    }
}

/// Ask the system for the post-event permission. This may show a prompt, so it runs only
/// from an explicit `request_permissions` while input is enabled, and never in a test.
func requestPostEventAccess(_ launch: SystemInputLaunch) {
    _ = CGRequestPostEventAccess()
}

func makeSystemInputBackend(_ launch: SystemInputLaunch) -> (sampler: InputSampler, poster: EventPoster) {
    let backend = SystemInputBackend()
    return (backend, backend)
}

private final class SystemInputBackend: InputSampler, EventPoster {
    // MARK: Sampling (read-only)

    func processStart(pid: Int32) -> Double? { kernelStart(pid) }

    func currentWindow(pid: Int32, windowId: UInt32) -> InputCurrentWindow? {
        guard let list = CGWindowListCopyWindowInfo([.optionIncludingWindow], CGWindowID(windowId)) as? [[String: Any]],
            let info = list.first,
            (info[kCGWindowNumber as String] as? NSNumber)?.uint32Value == windowId,
            (info[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value == pid,
            let bounds = info[kCGWindowBounds as String] as? NSDictionary,
            let rect = CGRect(dictionaryRepresentation: bounds),
            let start = kernelStart(pid)
        else { return nil }
        return InputCurrentWindow(
            pid: pid, windowId: windowId,
            frame: InputFrame(x: rect.origin.x, y: rect.origin.y, width: rect.width, height: rect.height),
            processStart: start)
    }

    /// The bundle id and the frontmost app (G8) are read-only queries. Every fact that is
    /// not sampled yet is false, so the gate refuses.
    func liveFacts(_ request: InputRequest, window: InputCurrentWindow?) -> InputLiveFacts {
        var facts = InputLiveFacts.failClosed
        facts.currentBundleId = NSRunningApplication(processIdentifier: request.pid)?.bundleIdentifier
        facts.targetFrontmost = NSWorkspace.shared.frontmostApplication?.processIdentifier == request.pid
        return facts
    }

    func verifyAfter(_ request: InputRequest) -> Bool {
        currentWindow(pid: request.pid, windowId: request.windowId) != nil
            && NSWorkspace.shared.frontmostApplication?.processIdentifier == request.pid
    }

    // MARK: Delivery. Reached only after every gate passed and the attempt receipt exists.

    private func screenPoint(_ x: Double, _ y: Double, _ window: InputCurrentWindow) -> CGPoint {
        CGPoint(x: window.frame.x + x, y: window.frame.y + y)
    }

    private func source() throws -> CGEventSource {
        guard let source = CGEventSource(stateID: .hidSystemState) else {
            throw PostError.notSent("no event source")
        }
        return source
    }

    private func mouse(_ type: CGEventType, _ x: Double, _ y: Double, _ window: InputCurrentWindow) throws {
        guard
            let event = CGEvent(
                mouseEventSource: try source(), mouseType: type,
                mouseCursorPosition: screenPoint(x, y, window), mouseButton: .left)
        else { throw PostError.notSent("event not created") }
        event.flags = []
        event.post(tap: .cghidEventTap)
    }

    private func key(_ keycode: Int, down: Bool) throws {
        guard let event = CGEvent(keyboardEventSource: try source(), virtualKey: CGKeyCode(keycode), keyDown: down)
        else { throw PostError.notSent("event not created") }
        event.flags = []
        event.post(tap: .cghidEventTap)
    }

    func movePointer(x: Double, y: Double, window: InputCurrentWindow) throws { try mouse(.mouseMoved, x, y, window) }
    func buttonDown(x: Double, y: Double, window: InputCurrentWindow) throws { try mouse(.leftMouseDown, x, y, window) }
    func buttonUp(x: Double, y: Double, window: InputCurrentWindow) throws { try mouse(.leftMouseUp, x, y, window) }
    func keyDown(_ keycode: Int) throws { try key(keycode, down: true) }
    func keyUp(_ keycode: Int) throws { try key(keycode, down: false) }
}
