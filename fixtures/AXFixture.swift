// Off-screen read-only fixture. It never requests activation or posts input.
import AppKit
import Foundation

let app = NSApplication.shared
if app.activationPolicy() != .accessory && !app.setActivationPolicy(.accessory) {
    fputs("cannot set fixture policy\n", stderr); exit(1)
}
guard app.activationPolicy() == .accessory else { fputs("fixture policy changed\n", stderr); exit(1) }
let window = NSWindow(contentRect: NSRect(x: -30000, y: -30000, width: 240, height: 140),
                      styleMask: [.titled], backing: .buffered, defer: false)
window.title = "Rust AX Fixture"
window.isReleasedWhenClosed = false
let button = NSButton(title: "Fixture Button", target: nil, action: nil)
button.frame = NSRect(x: 20, y: 40, width: 160, height: 32)
window.contentView?.addSubview(button)
// Do not order the fixture on screen. AppKit may reposition off-screen frames.
guard !window.isVisible else { fputs("fixture unexpectedly visible\n", stderr); exit(1) }
print("READY \(getpid())")
fflush(stdout)
DispatchQueue.main.asyncAfter(deadline: .now() + 120) { app.terminate(nil) }
app.run()
