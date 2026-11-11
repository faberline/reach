// The system input backend for ReachAgent (plan I-2, faberline/reach#27).
//
// This is the ONLY source file that may name an event delivery API or the post-event
// permission request; tests/foreground_input_static.rs scans every Swift file that the
// build scripts compile and fails if one appears anywhere else.
//
// It is compiled in. `SystemInputLaunch.authorize` yields a launch only when ReachAgent runs
// `--serve --enable-input` with its arming options, without `--test-mode`, and the process
// environment (read here, never passed in) has no non-empty `CI` variable. Its initializer is
// fileprivate, so no other file can make a `SystemInputLaunch`, and `makeSystemInputBackend` and
// `requestPostEventAccess` both require one.
//
// These checks, and the static tests that list every call site of `SystemInputLaunch.authorize(`
// and `makeInputController(`, are guards against mistakes, not a security boundary: anyone who
// can start ReachAgent with `--enable-input` and no `--test-mode` outside CI reaches this backend,
// and anyone who edits the source can edit the tests.
//
// What actually stops delivery today is that the live facts fail closed: focus, HID idle, held
// modifiers and mouse buttons, secure input, input source and occlusion are not fed into
// `liveFacts` (I-3 adds samplers but does not wire them in; that waits for I-4), so the gate
// refuses every request on this backend at G7, G8 or G9 before any event could be posted. Once I-4
// wires the samples in this no longer holds, and only the arming, allow-list, event-count and
// expiry gates and the user's TCC grant stand between a request and a posted event.
import AppKit
import CoreGraphics
import Foundation

/// Proof that start-up allowed the system input backend.
struct SystemInputLaunch {
    fileprivate init() {}

    static func authorize(enableInput: Bool, testMode: Bool) -> SystemInputLaunch? {
        guard enableInput, !testMode else { return nil }
        if let ci = ProcessInfo.processInfo.environment["CI"], !ci.isEmpty { return nil }
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

/// G9 sample of held modifiers and mouse buttons (plan review R2). Read-only: it reads the HID
/// system state, posts nothing and asks for no permission. The decisions are
/// `modifiersReleased(flags:)` and `mouseButtonsReleased(_:)` in ReachAgentInput.swift.
/// It is deliberately NOT called from `liveFacts` yet: both facts stay fail-closed until I-4
/// wires every sample in at once and verifies it on a real machine.
func sampleHeldInput() -> (modifiersReleased: Bool, mouseButtonsReleased: Bool) {
    let flags = CGEventSource.flagsState(.hidSystemState).rawValue
    let down = [CGMouseButton.left, .right, .center].map { CGEventSource.buttonState(.hidSystemState, button: $0) }
    return (modifiersReleased(flags: flags), mouseButtonsReleased(down))
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
