// ReachAgent read-only observations (plan 4.5, I-3): the current keyboard input source, the
// secure event input state, the pointer location and the on-screen windows of one bundle id.
//
// They only read state. No event is posted, no input source is selected or enabled, no
// permission is checked or requested, and nothing here can show a system prompt. Window titles
// are never read (they need Screen Recording).
//
// The input gate does not use these readings: the live facts of the system input backend stay
// fail-closed. Wiring them into the gate waits for the I-4 fixture verification on a real
// machine. None of the system readings below has been verified on a real machine yet.
import AppKit
import Carbon
import CoreGraphics
import Foundation

/// Most distinct owner processes and windows one `bundle_windows` report may list.
let maxObservedProcesses = 8
let maxObservedWindows = 64

struct ObservationFailure: Error {
    let message: String
    init(_ message: String) { self.message = message }
}

/// One on-screen window of `bundle_windows`.
struct ObservedWindow {
    let windowId: UInt32
    let ownerPid: Int32
    let layer: Int
    let x: Double, y: Double, width: Double, height: Double
    let alpha: Double
    /// Position in the whole on-screen window list, front (0) to back.
    let zIndex: Int

    var json: [String: Any] {
        [
            "window_id": Int(windowId), "owner_pid": Int(ownerPid), "layer": layer,
            "bounds": ["x": x, "y": y, "width": width, "height": height],
            "alpha": alpha, "z_index": zIndex,
        ]
    }
}

struct ObservedInputSource {
    let id: String
    let modeId: String?
    let localizedName: String?
    let sourceType: String?
}

/// Contract: read one piece of state without any side effect, or throw `ObservationFailure`.
protocol ObservationBackend: Sendable {
    var name: String { get }
    func inputSource() throws -> ObservedInputSource
    func secureEventInputEnabled() throws -> Bool
    /// Global display coordinates in points, origin at the top left of the main display.
    func pointerLocation() throws -> (x: Double, y: Double)
    func bundleWindows(bundleId: String) throws -> (ownerPids: [Int32], windows: [ObservedWindow], truncated: Bool)
}

// MARK: - Request handling

/// The methods without parameters accept no `params` or an empty object.
func parseNoObservationParams(_ value: Any?) throws {
    guard let value = value else { return }
    guard let object = value as? [String: Any] else {
        throw AgentError("invalid_params", "params must be an object")
    }
    if let extra = object.keys.sorted().first {
        throw AgentError("invalid_params", "unknown parameter `\(extra)`")
    }
}

/// 1 to 255 bytes of ASCII letters, digits, dots and hyphens (the `cu-agent` rule).
func validObservedBundleId(_ value: String) -> Bool {
    let bytes = Array(value.utf8)
    return (1...255).contains(bytes.count)
        && bytes.allSatisfy { byte in
            (0x30...0x39).contains(byte) || (0x41...0x5A).contains(byte) || (0x61...0x7A).contains(byte)
                || byte == 0x2E || byte == 0x2D
        }
}

/// Exactly `bundle_id`. The Rust `parse_bundle_windows_params` follows the same rules.
func parseBundleWindowsParams(_ value: Any?) throws -> String {
    guard let object = value as? [String: Any] else {
        throw AgentError("invalid_params", "params must be an object")
    }
    if let extra = object.keys.sorted().first(where: { $0 != "bundle_id" }) {
        throw AgentError("invalid_params", "unknown parameter `\(extra)`")
    }
    guard let bundleId = object["bundle_id"] as? String, validObservedBundleId(bundleId) else {
        throw AgentError(
            "invalid_params", "`bundle_id` must be 1 to 255 bytes of ASCII letters, digits, dots and hyphens")
    }
    return bundleId
}

/// `input_source`, `secure_input`, `pointer_location` and `bundle_windows`. Parameters are
/// checked before the backend is called; a backend failure is `observation_failed`.
func handleObservation(_ id: String, method: String, _ object: [String: Any], backend: ObservationBackend)
    -> String
{
    var result: [String: Any]
    do {
        switch method {
        case "input_source":
            try parseNoObservationParams(object["params"])
            let source = try backend.inputSource()
            result = [
                "input_source_id": source.id,
                "input_mode_id": source.modeId.map { $0 as Any } ?? NSNull(),
                "localized_name": source.localizedName.map { $0 as Any } ?? NSNull(),
                "source_type": source.sourceType.map { $0 as Any } ?? NSNull(),
                "selection_changed": false,
            ]
        case "secure_input":
            try parseNoObservationParams(object["params"])
            result = ["secure_event_input_enabled": try backend.secureEventInputEnabled()]
        case "pointer_location":
            try parseNoObservationParams(object["params"])
            let point = try backend.pointerLocation()
            guard point.x.isFinite, point.y.isFinite else { throw ObservationFailure("the pointer location is not finite") }
            result = ["x": point.x, "y": point.y, "coordinate_space": "global_top_left_points"]
        default:
            let bundleId = try parseBundleWindowsParams(object["params"])
            let found = try backend.bundleWindows(bundleId: bundleId)
            result = [
                "bundle_id": bundleId, "owner_pids": found.ownerPids.map { Int($0) },
                "windows": found.windows.map { $0.json }, "truncated": found.truncated,
                "window_scope": "onscreen_only", "window_titles": "not_read",
            ]
        }
    } catch let error as AgentError {
        return errorLine(id, error)
    } catch let failure as ObservationFailure {
        return errorLine(id, AgentError("observation_failed", failure.message))
    } catch {
        return errorLine(id, AgentError("observation_failed", "\(error)"))
    }
    result["backend"] = backend.name
    result["scope"] = "read_only_unverified"
    result["action_allowed"] = false
    result["used_by_input_gate"] = false
    return okLine(id, result)
}

// MARK: - System backend

/// Real readings. None of them has been verified on a real machine yet (I-3 acceptance).
struct SystemObservationBackend: ObservationBackend {
    var name: String { "system" }

    func inputSource() throws -> ObservedInputSource {
        // Text Input Sources must be read on the main thread; the service loop runs there.
        guard Thread.isMainThread else { throw ObservationFailure("the input source is read on the main thread only") }
        guard let source = TISCopyCurrentKeyboardInputSource()?.takeRetainedValue() else {
            throw ObservationFailure("no current keyboard input source")
        }
        func text(_ key: CFString) -> String? {
            guard let raw = TISGetInputSourceProperty(source, key) else { return nil }
            return Unmanaged<AnyObject>.fromOpaque(raw).takeUnretainedValue() as? String
        }
        guard let id = text(kTISPropertyInputSourceID) else {
            throw ObservationFailure("the current input source has no id")
        }
        return ObservedInputSource(
            id: id, modeId: text(kTISPropertyInputModeID), localizedName: text(kTISPropertyLocalizedName),
            sourceType: text(kTISPropertyInputSourceType))
    }

    func secureEventInputEnabled() throws -> Bool { IsSecureEventInputEnabled() }

    func pointerLocation() throws -> (x: Double, y: Double) {
        // AppKit reports bottom-left coordinates of the main display; flip them to the
        // top-left global space that window bounds use.
        let point = NSEvent.mouseLocation
        let main = CGDisplayBounds(CGMainDisplayID())
        guard main.height > 0 else { throw ObservationFailure("the main display has no bounds") }
        return (Double(point.x), Double(main.height - point.y))
    }

    func bundleWindows(bundleId: String) throws -> (ownerPids: [Int32], windows: [ObservedWindow], truncated: Bool) {
        guard
            let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
                as? [[String: Any]]
        else {
            throw ObservationFailure("the on-screen window list is unavailable")
        }
        // The owner of each window is looked up by pid on every call, so the answer does not
        // depend on a run loop keeping a cached application list fresh.
        var matches: [Int32: Bool] = [:]
        var owners: [Int32] = []
        var windows: [ObservedWindow] = []
        var truncated = false
        for (zIndex, info) in list.enumerated() {
            guard let pid = (info[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value, pid > 0 else { continue }
            let match =
                matches[pid] ?? (NSRunningApplication(processIdentifier: pid)?.bundleIdentifier == bundleId)
            matches[pid] = match
            guard match else { continue }
            guard let number = (info[kCGWindowNumber as String] as? NSNumber)?.uint32Value,
                let layer = (info[kCGWindowLayer as String] as? NSNumber)?.intValue,
                let boundsDictionary = info[kCGWindowBounds as String] as? NSDictionary,
                let bounds = CGRect(dictionaryRepresentation: boundsDictionary as CFDictionary)
            else { continue }
            if !owners.contains(pid) {
                if owners.count == maxObservedProcesses {
                    truncated = true
                    continue
                }
                owners.append(pid)
            }
            if windows.count == maxObservedWindows {
                truncated = true
                break
            }
            let alpha = (info[kCGWindowAlpha as String] as? NSNumber)?.doubleValue ?? 1
            windows.append(
                ObservedWindow(
                    windowId: number, ownerPid: pid, layer: layer, x: Double(bounds.origin.x),
                    y: Double(bounds.origin.y), width: Double(bounds.width), height: Double(bounds.height),
                    alpha: alpha, zIndex: zIndex))
        }
        return (owners.sorted(), windows, truncated)
    }
}

// MARK: - Fake backend (test mode and self-test)

/// Deterministic readings without any system call. The fixture bundle id has one process (pid
/// 42) with a layer-0 window and a layer-20 window; `org.local.reach.fake-failure` fails; any
/// other bundle id has no windows.
struct FakeObservationBackend: ObservationBackend {
    static let failingBundleId = "org.local.reach.fake-failure"

    var name: String { "fake" }

    func inputSource() throws -> ObservedInputSource {
        ObservedInputSource(
            id: "org.local.reach.fake-input-source", modeId: nil, localizedName: "Fake", sourceType: nil)
    }

    func secureEventInputEnabled() throws -> Bool { false }

    func pointerLocation() throws -> (x: Double, y: Double) { (100.5, 200.25) }

    func bundleWindows(bundleId: String) throws -> (ownerPids: [Int32], windows: [ObservedWindow], truncated: Bool) {
        if bundleId == FakeObservationBackend.failingBundleId {
            throw ObservationFailure("fake window list failure")
        }
        guard bundleId == fixtureBundleId else { return ([], [], false) }
        return (
            [42],
            [
                ObservedWindow(
                    windowId: 7, ownerPid: 42, layer: 0, x: 0, y: 25, width: 800, height: 600, alpha: 1, zIndex: 0),
                ObservedWindow(
                    windowId: 8, ownerPid: 42, layer: 20, x: 120, y: 300, width: 240, height: 40, alpha: 1, zIndex: 1),
            ],
            false
        )
    }
}
