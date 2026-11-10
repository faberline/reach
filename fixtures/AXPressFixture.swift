import AppKit
import Foundation

// This executable is only launched as a child of the paired local test runner.
// The nonce makes the AX button unique to that one run.
guard CommandLine.arguments.count == 4, CommandLine.arguments[1] == "--child-fixture",
      let expectedParent = Int32(CommandLine.arguments[2]), expectedParent == getppid(),
      let directory = ProcessInfo.processInfo.environment["AX_FIXTURE_PRIVATE_DIR"],
      CommandLine.arguments[3].count == 36 else { exit(2) }
let nonce = CommandLine.arguments[3]
let directoryURL = URL(fileURLWithPath: directory, isDirectory: true)
let expectedDirectory = URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
    .appendingPathComponent("ax-press-\(nonce)").standardizedFileURL
guard directoryURL.standardizedFileURL == expectedDirectory else { exit(2) }

final class Fixture: NSObject {
    let directoryURL: URL
    var count = 0
    init(_ directory: URL) { directoryURL = directory }
    @objc func pressed(_ sender: Any?) {
        count += 1
        try? Data("pressed".utf8).write(to: directoryURL.appendingPathComponent("count-\(count)"), options: .withoutOverwriting)
    }
}

let app = NSApplication.shared
guard app.setActivationPolicy(.accessory), let screen = NSScreen.main else { exit(3) }
let frame = screen.visibleFrame
let window = NSWindow(contentRect: NSRect(x: frame.maxX - 260, y: frame.maxY - 150,
                                         width: 240, height: 110),
                      styleMask: [.titled], backing: .buffered, defer: false)
window.title = "AX Press Fixture \(nonce)"
window.isReleasedWhenClosed = false
let fixture = Fixture(directoryURL)
let button = NSButton(title: "Fixture Button \(nonce)", target: fixture, action: #selector(Fixture.pressed(_:)))
button.setAccessibilityIdentifier("ax-press-fixture-\(nonce)")
button.frame = NSRect(x: 20, y: 35, width: 200, height: 32)
window.contentView?.addSubview(button)
window.orderBack(nil) // A real visible window can still cover content; never run by default.
do {
    try Data("ready".utf8).write(to: directoryURL.appendingPathComponent("ready"), options: .withoutOverwriting)
} catch { exit(4) }
DispatchQueue.main.asyncAfter(deadline: .now() + 15) { app.terminate(nil) }
app.run()
