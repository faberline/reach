import AppKit
import Foundation

// This child accepts only a parent PID and an unpredictable run-specific nonce.
guard CommandLine.arguments.count == 4, CommandLine.arguments[1] == "--child-fixture",
      let parent = Int32(CommandLine.arguments[2]), parent == getppid(),
      let nonce = UUID(uuidString: CommandLine.arguments[3]),
      let path = ProcessInfo.processInfo.environment["TARGET_EVENT_FIXTURE_DIR"] else { exit(2) }
let directory = URL(fileURLWithPath: path, isDirectory: true).standardizedFileURL
let expected = URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
    .appendingPathComponent("target-event-\(nonce.uuidString)").standardizedFileURL
guard directory == expected else { exit(2) }

final class EventView: NSView {
    let directory: URL
    var downs = 0
    var ups = 0
    init(directory: URL, frame: NSRect) { self.directory = directory; super.init(frame: frame) }
    required init?(coder: NSCoder) { nil }
    override var acceptsFirstResponder: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override func draw(_ dirtyRect: NSRect) {
        NSColor.systemBlue.withAlphaComponent(0.25).setFill()
        dirtyRect.fill()
    }
    func record(_ event: NSEvent, kind: String, count: Int) {
        let v: [String: Any] = ["kind": kind, "count": count, "pid": getpid(),
            "window_number": event.windowNumber, "x": event.locationInWindow.x,
            "y": event.locationInWindow.y]
        guard let data = try? JSONSerialization.data(withJSONObject: v, options: [.sortedKeys]) else { return }
        try? data.write(to: directory.appendingPathComponent("\(kind)-\(count).json"), options: .withoutOverwriting)
    }
    override func mouseDown(with event: NSEvent) { downs += 1; record(event, kind: "down", count: downs) }
    override func mouseUp(with event: NSEvent) { ups += 1; record(event, kind: "up", count: ups) }
}

let app = NSApplication.shared
if app.activationPolicy() != .accessory { _ = app.setActivationPolicy(.accessory) }
guard app.activationPolicy() == .accessory else { fputs("activation_policy_not_accessory\n", stderr); exit(3) }
guard let screen = NSScreen.main else { fputs("main_screen_unavailable\n", stderr); exit(3) }
let frame = screen.visibleFrame
final class NonActivatingPanel: NSPanel {
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
}
let window = NonActivatingPanel(contentRect: NSRect(x: frame.maxX - 280, y: frame.maxY - 170,
    width: 240, height: 110), styleMask: [.titled, .nonactivatingPanel], backing: .buffered, defer: false)
window.title = "Target Event Fixture \(nonce.uuidString)"
window.isReleasedWhenClosed = false
let view = EventView(directory: directory, frame: NSRect(x: 0, y: 0, width: 240, height: 110))
window.contentView = view
let hidden = ProcessInfo.processInfo.environment["TARGET_EVENT_FIXTURE_HIDDEN"] == "1"
let ignoresMouse = ProcessInfo.processInfo.environment["TARGET_EVENT_FIXTURE_IGNORE_MOUSE"] == "1"
window.ignoresMouseEvents = ignoresMouse
if !hidden { window.orderBack(nil) }
if hidden && window.isVisible { fputs("hidden_fixture_visible\n", stderr); exit(5) }
// AppKit uses bottom-left screen coordinates; CG uses a top-left main-display origin.
// This reported frame is an unverified hint until independently matched by the runner.
let appkitFrame = window.frame
let quartzFrame: [String: Double] = ["x": Double(appkitFrame.minX),
    "y": Double(CGDisplayBounds(CGMainDisplayID()).height - appkitFrame.maxY),
    "width": Double(appkitFrame.width), "height": Double(appkitFrame.height)]
let ready: [String: Any] = ["pid": getpid(), "appkit_window_number": window.windowNumber,
                            "quartz_frame": quartzFrame, "nonce": nonce.uuidString, "visible": window.isVisible, "ignores_mouse": window.ignoresMouseEvents]
do {
    let data = try JSONSerialization.data(withJSONObject: ready, options: [.sortedKeys])
    try data.write(to: directory.appendingPathComponent("ready.json"), options: .withoutOverwriting)
} catch { exit(4) }
DispatchQueue.main.asyncAfter(deadline: .now() + 15) { app.terminate(nil) }
app.run()
