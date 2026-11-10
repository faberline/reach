// Display-only cursor overlay. It never posts events, never moves the real
// cursor, never takes focus and needs no TCC permission. A shown overlay is
// not bound to any window and never authorizes input.
// --self-test is pure data plus one panel that is never ordered on screen.
import AppKit

let overlayProtocol = "cursor-overlay-v1"
let pointSpace = "global_display_points_top_left"
let maxCoordinate = 1_000_000.0
let phases = ["idle", "click", "hold", "drag"]
let overlaySize = 28.0

/// A display in global points with the origin at the top left of the primary display.
struct Screen { let id: UInt32; let x, y, width, height: Double }
struct OverlayPoint: Equatable { let displayID: UInt32; let x, y: Double }
struct OverlayState: Equatable { var visible = false; var phase = "idle"; var point: OverlayPoint? }
struct Refusal: Error, Equatable { let reason: String }
enum Command: Equatable { case show(OverlayPoint), move(OverlayPoint), phase(String), hide, query, quit }

func number(_ any: Any?) -> Double? {
    guard let n = any as? NSNumber, CFGetTypeID(n) != CFBooleanGetTypeID(), n.doubleValue.isFinite else { return nil }
    return n.doubleValue
}

func parsePoint(_ any: Any?) throws -> OverlayPoint {
    guard let o = any as? [String: Any], Set(o.keys) == ["space", "display_id", "x", "y"],
          o["space"] as? String == pointSpace,
          let d = number(o["display_id"]), d >= 1, d <= Double(UInt32.max), d == d.rounded(),
          let x = number(o["x"]), let y = number(o["y"]),
          abs(x) <= maxCoordinate, abs(y) <= maxCoordinate else { throw Refusal(reason: "invalid_point") }
    return OverlayPoint(displayID: UInt32(d), x: x, y: y)
}

func validID(_ any: Any?) -> String? {
    guard let id = any as? String, (1...64).contains(id.utf8.count),
          id.utf8.allSatisfy({ (0x21...0x7e).contains($0) }) else { return nil }
    return id
}

func parseRequest(_ line: String) throws -> (id: String, command: Command) {
    guard let o = (try? JSONSerialization.jsonObject(with: Data(line.utf8))) as? [String: Any] else {
        throw Refusal(reason: "invalid_json")
    }
    guard o["protocol"] as? String == overlayProtocol, let id = validID(o["id"]),
          let name = o["command"] as? String else { throw Refusal(reason: "invalid_request") }
    let extra: String?
    switch name {
    case "show", "move": extra = "point"
    case "phase": extra = "phase"
    case "hide", "query", "quit": extra = nil
    default: throw Refusal(reason: "unknown_command")
    }
    var keys: Set<String> = ["protocol", "id", "command"]
    if let extra { keys.insert(extra) }
    guard Set(o.keys) == keys else { throw Refusal(reason: "invalid_request") }
    switch name {
    case "show": return (id, .show(try parsePoint(o["point"])))
    case "move": return (id, .move(try parsePoint(o["point"])))
    case "phase":
        guard let p = o["phase"] as? String, phases.contains(p) else { throw Refusal(reason: "invalid_phase") }
        return (id, .phase(p))
    case "hide": return (id, .hide)
    case "query": return (id, .query)
    default: return (id, .quit)
    }
}

func checkOnDisplay(_ p: OverlayPoint, _ screens: [Screen]) throws {
    guard let s = screens.first(where: { $0.id == p.displayID }) else { throw Refusal(reason: "unknown_display") }
    guard p.x >= s.x, p.x < s.x + s.width, p.y >= s.y, p.y < s.y + s.height else {
        throw Refusal(reason: "point_outside_display")
    }
}

func apply(_ state: OverlayState, _ command: Command, screens: [Screen]) throws -> OverlayState {
    switch command {
    case .show(let p):
        try checkOnDisplay(p, screens)
        return OverlayState(visible: true, phase: "idle", point: p)
    case .move(let p):
        guard state.visible else { throw Refusal(reason: "not_visible") }
        try checkOnDisplay(p, screens)
        return OverlayState(visible: true, phase: state.phase, point: p)
    case .phase(let phase):
        guard state.visible else { throw Refusal(reason: "not_visible") }
        return OverlayState(visible: true, phase: phase, point: state.point)
    case .hide, .quit: return OverlayState()
    case .query: return state
    }
}

func reply(id: String?, state: OverlayState, reason: String?) -> [String: Any] {
    let point: Any = state.point.map {
        ["space": pointSpace, "display_id": Int($0.displayID), "x": $0.x, "y": $0.y] as [String: Any]
    } ?? NSNull()
    return ["protocol": overlayProtocol, "id": id ?? NSNull(), "status": reason == nil ? "ok" : "error",
            "reason": reason ?? NSNull(), "visible": state.visible, "phase": state.phase, "point": point,
            "window_binding": "unbound", "action_allowed": false]
}

/// AppKit origin (bottom left) of a square panel centred on a top-left global point.
func windowOrigin(_ p: OverlayPoint, primaryHeight: Double, size: Double) -> (x: Double, y: Double) {
    (p.x - size / 2, primaryHeight - p.y - size / 2)
}

final class OverlayPanel: NSPanel {
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
}

final class OverlayView: NSView {
    var phase = "idle" { didSet { needsDisplay = true } }
    override func draw(_ dirtyRect: NSRect) {
        let ring = NSBezierPath(ovalIn: bounds.insetBy(dx: 3, dy: 3))
        let color: NSColor = phase == "idle" ? .systemBlue : phase == "click" ? .systemOrange : .systemRed
        if phase != "idle" { color.withAlphaComponent(0.35).setFill(); ring.fill() }
        color.setStroke()
        ring.lineWidth = phase == "drag" ? 4 : 2
        ring.stroke()
    }
}

func makePanel() -> OverlayPanel {
    let panel = OverlayPanel(contentRect: NSRect(x: 0, y: 0, width: overlaySize, height: overlaySize),
                             styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: true)
    panel.ignoresMouseEvents = true
    panel.level = .screenSaver
    panel.isOpaque = false
    panel.backgroundColor = .clear
    panel.hasShadow = false
    panel.hidesOnDeactivate = false
    panel.isReleasedWhenClosed = false
    panel.collectionBehavior = [.canJoinAllSpaces, .stationary, .ignoresCycle, .fullScreenAuxiliary]
    panel.contentView = OverlayView(frame: NSRect(x: 0, y: 0, width: overlaySize, height: overlaySize))
    return panel
}

func currentScreens() -> [Screen] {
    let primaryHeight = Double(NSScreen.screens.first?.frame.height ?? 0)
    return NSScreen.screens.compactMap { s in
        guard let n = s.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber else { return nil }
        let f = s.frame
        return Screen(id: n.uint32Value, x: Double(f.minX), y: primaryHeight - Double(f.maxY),
                      width: Double(f.width), height: Double(f.height))
    }
}

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data("self-test failed: \(message)\n".utf8))
    exit(1)
}

func refusal(_ body: () throws -> Void) -> String? {
    do { try body(); return nil } catch let r as Refusal { return r.reason } catch { return "other" }
}

func selfTest() {
    let head = #""protocol":"cursor-overlay-v1","id":"a","#
    let point = #"{"space":"global_display_points_top_left","display_id":1,"x":10.5,"y":20}"#
    guard let shown = try? parseRequest("{\(head)\"command\":\"show\",\"point\":\(point)}"),
          shown.id == "a", shown.command == .show(OverlayPoint(displayID: 1, x: 10.5, y: 20)) else { fail("show") }
    guard (try? parseRequest("{\(head)\"command\":\"phase\",\"phase\":\"drag\"}"))?.command == .phase("drag"),
          (try? parseRequest("{\(head)\"command\":\"quit\"}"))?.command == .quit else { fail("phase or quit") }
    let refused: [(String, String)] = [
        ("not json", "invalid_json"),
        ("[]", "invalid_json"),
        (#"{"protocol":"cursor-overlay-v0","id":"a","command":"hide"}"#, "invalid_request"),
        (#"{"protocol":"cursor-overlay-v1","id":"","command":"hide"}"#, "invalid_request"),
        (#"{"protocol":"cursor-overlay-v1","id":"a b","command":"hide"}"#, "invalid_request"),
        ("{\(head)\"command\":\"hide\",\"extra\":1}", "invalid_request"),
        ("{\(head)\"command\":\"show\"}", "invalid_request"),
        ("{\(head)\"command\":\"hide\",\"point\":\(point)}", "invalid_request"),
        ("{\(head)\"command\":\"click\"}", "unknown_command"),
        ("{\(head)\"command\":\"phase\",\"phase\":\"pressed\"}", "invalid_phase"),
        ("{\(head)\"command\":\"show\",\"point\":{\"space\":\"window\",\"display_id\":1,\"x\":1,\"y\":1}}", "invalid_point"),
        ("{\(head)\"command\":\"show\",\"point\":{\"space\":\"\(pointSpace)\",\"display_id\":0,\"x\":1,\"y\":1}}", "invalid_point"),
        ("{\(head)\"command\":\"show\",\"point\":{\"space\":\"\(pointSpace)\",\"display_id\":1.5,\"x\":1,\"y\":1}}", "invalid_point"),
        ("{\(head)\"command\":\"show\",\"point\":{\"space\":\"\(pointSpace)\",\"display_id\":1,\"x\":true,\"y\":1}}", "invalid_point"),
        ("{\(head)\"command\":\"show\",\"point\":{\"space\":\"\(pointSpace)\",\"display_id\":1,\"x\":1000001,\"y\":1}}", "invalid_point"),
        ("{\(head)\"command\":\"show\",\"point\":{\"space\":\"\(pointSpace)\",\"display_id\":1,\"x\":1,\"y\":1,\"z\":0}}", "invalid_point"),
    ]
    for (line, reason) in refused where refusal({ _ = try parseRequest(line) }) != reason {
        fail("request \(line) must be refused with \(reason)")
    }

    let screens = [Screen(id: 1, x: 0, y: 0, width: 100, height: 50), Screen(id: 2, x: 100, y: 0, width: 100, height: 50)]
    let a = OverlayPoint(displayID: 1, x: 10, y: 20), b = OverlayPoint(displayID: 2, x: 150, y: 49.5)
    let hidden = OverlayState()
    for command in [Command.move(a), .phase("hold")] where refusal({ _ = try apply(hidden, command, screens: screens) }) != "not_visible" {
        fail("hidden overlay must refuse \(command)")
    }
    guard refusal({ _ = try apply(hidden, .show(OverlayPoint(displayID: 3, x: 1, y: 1)), screens: screens) }) == "unknown_display",
          refusal({ _ = try apply(hidden, .show(OverlayPoint(displayID: 1, x: 100, y: 1)), screens: screens) }) == "point_outside_display",
          refusal({ _ = try apply(hidden, .show(OverlayPoint(displayID: 2, x: 150, y: 50)), screens: screens) }) == "point_outside_display",
          refusal({ _ = try apply(hidden, .show(OverlayPoint(displayID: 1, x: -1, y: 1)), screens: screens) }) == "point_outside_display"
    else { fail("display checks") }
    guard let s1 = try? apply(hidden, .show(a), screens: screens), s1 == OverlayState(visible: true, phase: "idle", point: a),
          let s2 = try? apply(s1, .phase("hold"), screens: screens), s2 == OverlayState(visible: true, phase: "hold", point: a),
          let s3 = try? apply(s2, .move(b), screens: screens), s3 == OverlayState(visible: true, phase: "hold", point: b),
          (try? apply(s3, .query, screens: screens)) == s3,
          (try? apply(s3, .show(a), screens: screens)) == s1,
          (try? apply(s3, .hide, screens: screens)) == hidden,
          (try? apply(s3, .quit, screens: screens)) == hidden,
          (try? apply(hidden, .hide, screens: screens)) == hidden else { fail("transitions") }

    let ok = reply(id: "a", state: s1, reason: nil), bad = reply(id: nil, state: hidden, reason: "invalid_json")
    guard ok.count == 9, ok["status"] as? String == "ok", ok["reason"] is NSNull, ok["visible"] as? Bool == true,
          ok["window_binding"] as? String == "unbound", ok["action_allowed"] as? Bool == false,
          (ok["point"] as? [String: Any])?["space"] as? String == pointSpace,
          bad.count == 9, bad["id"] is NSNull, bad["status"] as? String == "error", bad["point"] is NSNull,
          bad["reason"] as? String == "invalid_json", bad["window_binding"] as? String == "unbound",
          bad["action_allowed"] as? Bool == false, JSONSerialization.isValidJSONObject(ok) else { fail("reply") }
    let origin = windowOrigin(OverlayPoint(displayID: 1, x: 100, y: 30), primaryHeight: 900, size: 28)
    guard origin.x == 86, origin.y == 856 else { fail("origin") }

    // The panel is created but never ordered on screen.
    _ = NSApplication.shared
    let panel = makePanel()
    guard panel.ignoresMouseEvents, !panel.canBecomeKey, !panel.canBecomeMain, !panel.isVisible,
          panel.styleMask.contains(.nonactivatingPanel), !panel.styleMask.contains(.titled),
          panel.level == .screenSaver, !panel.isOpaque, !panel.hidesOnDeactivate,
          panel.contentView is OverlayView, !NSApplication.shared.isActive else { fail("panel attributes") }
    print(#"{"status":"self_test_passed","window_shown":false}"#)
}

func emit(_ object: [String: Any]) {
    guard let data = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys]) else { return }
    FileHandle.standardOutput.write(data + Data("\n".utf8))
}

func serve() -> Never {
    let app = NSApplication.shared
    app.setActivationPolicy(.accessory)
    let panel = makePanel()
    var state = OverlayState()
    func render() {
        guard state.visible, let p = state.point else { panel.orderOut(nil); return }
        (panel.contentView as? OverlayView)?.phase = state.phase
        let o = windowOrigin(p, primaryHeight: Double(NSScreen.screens.first?.frame.height ?? 0), size: overlaySize)
        panel.setFrameOrigin(NSPoint(x: o.x, y: o.y))
        panel.orderFrontRegardless()
    }
    // A display change makes the shown position unreliable: hide and report it unasked.
    _ = NotificationCenter.default.addObserver(
        forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main
    ) { _ in
        guard state.visible else { return }
        state = OverlayState()
        render()
        emit(reply(id: nil, state: state, reason: "display_changed"))
    }
    DispatchQueue.global().async {
        // End of input hides the overlay and stops the helper.
        while let line = readLine(strippingNewline: true) {
            let stop: Bool = DispatchQueue.main.sync {
                guard line.utf8.count <= 4096 else { emit(reply(id: nil, state: state, reason: "invalid_json")); return false }
                do {
                    let request = try parseRequest(line)
                    do {
                        state = try apply(state, request.command, screens: currentScreens())
                        render()
                        emit(reply(id: request.id, state: state, reason: nil))
                        return request.command == .quit
                    } catch let r as Refusal { emit(reply(id: request.id, state: state, reason: r.reason)) } catch {}
                } catch let r as Refusal { emit(reply(id: nil, state: state, reason: r.reason)) } catch {}
                return false
            }
            if stop { break }
        }
        DispatchQueue.main.async { panel.orderOut(nil); exit(0) }
    }
    app.run()
    exit(0)
}

switch Array(CommandLine.arguments.dropFirst()) {
case ["--self-test"]: selfTest()
case ["--serve"]: serve()
default:
    FileHandle.standardError.write(Data("use --self-test or --serve\n".utf8))
    exit(2)
}
