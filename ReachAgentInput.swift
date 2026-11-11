// Foreground input for the ReachAgent service (plan I-2, faberline/reach#27). It is compiled
// together with ReachAgent.swift.
//
// This file holds the arming options, the request parser, the gate, the receipts and the fake
// backend. It never delivers an event: the only code that may call an event delivery API is
// ReachAgentInputSystem.swift, and a static test (tests/foreground_input_static.rs) enforces it.
// The gate is a port of the offline Rust model in src/foreground_input.rs (plan I-1): the same
// order, the same gate names and the same reason strings.
//
// Input is disabled by default. Nothing here runs unless ReachAgent starts with --enable-input.
import CryptoKit
import Foundation

/// Methods added to `describe` only while input is enabled.
let inputMethods = ["action"]
/// The only bundle ids an arming may name (plan Q4). Chrome is not approved.
let approvedInputBundleIds = ["org.local.rust-computer-use.target-event-fixture", "com.apple.Safari"]
let maxInputArmSeconds = 600
let maxInputEvents = 20
let maxInputKeycode = 127
let maxObservationAgeNs: UInt64 = 10_000_000_000
let inputAttemptSchema = "reach-input-attempt-v1"
let inputResultSchema = "reach-input-result-v1"

// MARK: - Start-up options

struct InputOptions: Equatable {
    var bundles: [String] = []
    var keycodes: [Int] = []
    var ttlSeconds: Int = 0
    var maxEvents: Int = 0
}

struct ServeConfig: Equatable {
    let socket: String
    let testMode: Bool
    let input: InputOptions?
}

struct UsageError: Error, CustomStringConvertible {
    let description: String
    init(_ message: String) { description = message }
}

/// `--serve --socket <path> [--test-mode] [--enable-input --allow-bundle <id>...
/// [--allow-keycode <n>...] --input-ttl-seconds <n> --max-input-events <n>]`.
/// An input option without `--enable-input` is a usage error, and so is anything unknown.
func parseServeArguments(_ args: [String]) throws -> ServeConfig {
    guard args.count >= 3, args[0] == "--serve", args[1] == "--socket" else {
        throw UsageError("expected --serve --socket <path>")
    }
    var testMode = false
    var enable = false
    var options = InputOptions()
    var ttl: Int?
    var events: Int?
    var index = 3
    func value(_ flag: String) throws -> String {
        guard index + 1 < args.count else { throw UsageError("\(flag) needs a value") }
        index += 1
        return args[index]
    }
    func integer(_ flag: String, _ range: ClosedRange<Int>) throws -> Int {
        let text = try value(flag)
        guard text.allSatisfy({ $0.isASCII && $0.isNumber }), text.count <= 4, let n = Int(text),
            range.contains(n)
        else {
            throw UsageError("\(flag) must be an integer from \(range.lowerBound) to \(range.upperBound)")
        }
        return n
    }
    while index < args.count {
        let flag = args[index]
        switch flag {
        case "--test-mode":
            if testMode { throw UsageError("--test-mode appears twice") }
            testMode = true
        case "--enable-input":
            if enable { throw UsageError("--enable-input appears twice") }
            enable = true
        case "--allow-bundle":
            let bundle = try value(flag)
            guard approvedInputBundleIds.contains(bundle) else {
                throw UsageError("--allow-bundle accepts only \(approvedInputBundleIds.joined(separator: ", "))")
            }
            if options.bundles.contains(bundle) { throw UsageError("a bundle id appears twice") }
            options.bundles.append(bundle)
        case "--allow-keycode":
            let keycode = try integer(flag, 0...maxInputKeycode)
            if options.keycodes.contains(keycode) { throw UsageError("a keycode appears twice") }
            options.keycodes.append(keycode)
        case "--input-ttl-seconds":
            if ttl != nil { throw UsageError("--input-ttl-seconds appears twice") }
            ttl = try integer(flag, 1...maxInputArmSeconds)
        case "--max-input-events":
            if events != nil { throw UsageError("--max-input-events appears twice") }
            events = try integer(flag, 1...maxInputEvents)
        default:
            throw UsageError("unknown argument \(flag)")
        }
        index += 1
    }
    let anyInputOption = !options.bundles.isEmpty || !options.keycodes.isEmpty || ttl != nil || events != nil
    if !enable {
        if anyInputOption { throw UsageError("input options need --enable-input") }
        return ServeConfig(socket: args[2], testMode: testMode, input: nil)
    }
    guard !options.bundles.isEmpty else { throw UsageError("--enable-input needs at least one --allow-bundle") }
    guard let ttl = ttl else { throw UsageError("--enable-input needs --input-ttl-seconds") }
    guard let events = events else { throw UsageError("--enable-input needs --max-input-events") }
    options.ttlSeconds = ttl
    options.maxEvents = events
    return ServeConfig(socket: args[2], testMode: testMode, input: options)
}

// MARK: - Arming (G1, G2)

struct InputArming {
    let bundles: [String]
    let keycodes: Set<Int>
    let armedAtNs: UInt64
    let expiresAtNs: UInt64
    let expiresUnixSeconds: Double
    var remaining: Int

    /// The same limits as `Arming::new` in the Rust model; nil when any is broken.
    init?(_ options: InputOptions, nowNs: UInt64, nowUnixSeconds: Double) {
        guard !options.bundles.isEmpty,
            options.bundles.allSatisfy({ approvedInputBundleIds.contains($0) }),
            options.keycodes.allSatisfy({ (0...maxInputKeycode).contains($0) }),
            (1...maxInputArmSeconds).contains(options.ttlSeconds),
            (1...maxInputEvents).contains(options.maxEvents)
        else { return nil }
        bundles = options.bundles
        keycodes = Set(options.keycodes)
        armedAtNs = nowNs
        expiresAtNs = nowNs + UInt64(options.ttlSeconds) * 1_000_000_000
        expiresUnixSeconds = nowUnixSeconds + Double(options.ttlSeconds)
        remaining = options.maxEvents
    }

    func expired(_ nowNs: UInt64) -> Bool { nowNs < armedAtNs || nowNs > expiresAtNs }
}

// MARK: - Request (plan 4.3)

enum InputEvent: Equatable {
    case pointerMove(x: Double, y: Double)
    case click(x: Double, y: Double)
    case keyTap(keycode: Int)

    var point: (x: Double, y: Double)? {
        switch self {
        case .pointerMove(let x, let y), .click(let x, let y): return (x, y)
        case .keyTap: return nil
        }
    }

    var json: [String: Any] {
        switch self {
        case .pointerMove(let x, let y): return ["kind": "pointer_move", "x": x, "y": y]
        case .click(let x, let y): return ["kind": "click", "button": "left", "x": x, "y": y]
        case .keyTap(let keycode): return ["kind": "key_tap", "keycode": keycode]
        }
    }
}

struct InputRequest: Equatable {
    let id: String
    let bundleId: String
    let pid: Int32
    let windowId: UInt32
    let processStart: Double
    let sha256: String
    let event: InputEvent
    let evidenceDir: String
}

struct InputRejection: Error, Equatable {
    let status: String
    let gate: String
    let reason: String
}

func schemaRejection(_ reason: String) -> InputRejection {
    InputRejection(status: "refused", gate: "schema", reason: reason)
}

func plainNumber(_ value: Any?) -> Double? {
    guard let number = value as? NSNumber, CFGetTypeID(number) != CFBooleanGetTypeID() else {
        return nil
    }
    let d = number.doubleValue
    return d.isFinite ? d : nil
}

func validInputText(_ value: Any?, max: Int) -> String? {
    guard let s = value as? String, !s.isEmpty, s.utf8.count <= max,
        !s.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) })
    else { return nil }
    return s
}

/// Strict parse of the `action` params. The same rules as `parse_request` in the Rust model:
/// unknown fields are refused, `background` is unsupported, nothing else is accepted.
func parseActionParams(id: String, _ params: Any?) -> Result<InputRequest, InputRejection> {
    func object(_ value: Any?, _ name: String, _ allowed: [String]) throws -> [String: Any] {
        guard let o = value as? [String: Any] else { throw schemaRejection("\(name) must be an object") }
        if let k = o.keys.sorted().first(where: { !allowed.contains($0) }) {
            throw schemaRejection("\(name): unknown field \"\(k)\"")
        }
        return o
    }
    func number(_ o: [String: Any], _ name: String) throws -> Double {
        guard let n = plainNumber(o[name]) else { throw schemaRejection("event.\(name) must be a finite number") }
        return n
    }
    do {
        let top = try object(params, "params", ["delivery", "target", "observation", "event", "evidence_dir"])
        switch top["delivery"] as? String {
        case "foreground": break
        case "background":
            throw InputRejection(status: "unsupported", gate: "delivery", reason: "background delivery is not supported")
        default: throw schemaRejection("delivery must be \"foreground\"")
        }
        guard validInputText(id, max: 128) != nil else { throw schemaRejection("request.id is invalid") }
        let t = try object(top["target"], "target", ["bundle_id", "pid", "window_id", "process_start_unix_seconds"])
        guard let bundle = validInputText(t["bundle_id"], max: 255) else {
            throw schemaRejection("target.bundle_id is invalid")
        }
        guard bundle.unicodeScalars.allSatisfy({ $0.isASCII && (CharacterSet.alphanumerics.contains($0) || ".-_".unicodeScalars.contains($0)) }) else {
            throw schemaRejection("target.bundle_id has invalid characters")
        }
        guard let pid = isPlainInteger(t["pid"]), (1...Int(Int32.max)).contains(pid) else {
            throw schemaRejection("target.pid must be positive")
        }
        guard let window = isPlainInteger(t["window_id"]), (1...Int(UInt32.max)).contains(window) else {
            throw schemaRejection("target.window_id must be positive")
        }
        guard let start = plainNumber(t["process_start_unix_seconds"]) else {
            throw schemaRejection("target.process_start_unix_seconds must be a finite number")
        }
        guard start > 0 else { throw schemaRejection("target.process_start_unix_seconds must be positive") }
        let o = try object(top["observation"], "observation", ["image_sha256"])
        guard let sha = o["image_sha256"] as? String, sha.utf8.count == 64,
            sha.utf8.allSatisfy({ (48...57).contains($0) || (97...102).contains($0) })
        else { throw schemaRejection("observation.image_sha256 must be 64 lowercase hex digits") }
        guard let dir = validInputText(top["evidence_dir"], max: 1024) else {
            throw schemaRejection("request.evidence_dir is invalid")
        }
        guard dir.hasPrefix("/") else { throw schemaRejection("evidence_dir must be an absolute path") }
        guard let e = top["event"] as? [String: Any] else { throw schemaRejection("event must be an object") }
        guard let kind = e["kind"] as? String else { throw schemaRejection("event.kind is required") }
        let event: InputEvent
        switch kind {
        case "pointer_move":
            let o = try object(e, "event", ["kind", "x", "y"])
            event = .pointerMove(x: try number(o, "x"), y: try number(o, "y"))
        case "click":
            let o = try object(e, "event", ["kind", "button", "x", "y"])
            guard o["button"] as? String == "left" else { throw schemaRejection("event.button must be \"left\"") }
            event = .click(x: try number(o, "x"), y: try number(o, "y"))
        case "key_tap":
            let o = try object(e, "event", ["kind", "keycode"])
            guard let keycode = isPlainInteger(o["keycode"]), keycode <= maxInputKeycode else {
                throw schemaRejection("event.keycode must be 0 through 127")
            }
            event = .keyTap(keycode: keycode)
        default:
            throw schemaRejection("unsupported event kind \"\(kind)\"")
        }
        return .success(
            InputRequest(
                id: id, bundleId: bundle, pid: Int32(pid), windowId: UInt32(window), processStart: start,
                sha256: sha, event: event, evidenceDir: dir))
    } catch let rejection as InputRejection {
        return .failure(rejection)
    } catch {
        return .failure(schemaRejection("\(error)"))
    }
}

// MARK: - Facts the gate consumes. ReachAgent samples them; a request never carries them.

struct InputFrame: Equatable {
    let x: Double
    let y: Double
    let width: Double
    let height: Double
}

/// One capture that ReachAgent itself made and hashed (G5).
struct InputObservation: Equatable {
    let sha256: String
    let pid: Int32
    let windowId: UInt32
    let frame: InputFrame
    let processStart: Double
    let capturedAtNs: UInt64
}

/// The target window as ReachAgent samples it right before the gate runs (G3).
struct InputCurrentWindow: Equatable {
    let pid: Int32
    let windowId: UInt32
    let frame: InputFrame
    let processStart: Double
}

/// Live facts for G3 and G7 to G9. A sampler that cannot establish a fact reports false.
struct InputLiveFacts: Equatable {
    var currentBundleId: String?
    var targetFrontmost: Bool
    var focusUnchanged: Bool
    var hidIdle: Bool
    var secureInputOff: Bool
    var inputSourceOk: Bool
    var pointUnoccluded: Bool

    static let failClosed = InputLiveFacts(
        currentBundleId: nil, targetFrontmost: false, focusUnchanged: false, hidIdle: false,
        secureInputOff: false, inputSourceOk: false, pointUnoccluded: false)
}

/// Read-only sampling. No method may deliver an event or ask for a permission.
protocol InputSampler: AnyObject {
    func processStart(pid: Int32) -> Double?
    func currentWindow(pid: Int32, windowId: UInt32) -> InputCurrentWindow?
    func liveFacts(_ request: InputRequest, window: InputCurrentWindow?) -> InputLiveFacts
    /// The post-event check (G3 after delivery). False means `delivery_unknown`.
    func verifyAfter(_ request: InputRequest) -> Bool
}

enum PostError: Error, Equatable {
    /// The backend is certain the OS received nothing.
    case notSent(String)
    /// Anything else, including every uncertain failure.
    case maybeSent(String)
}

/// Event delivery. Points are window points of `window`; the backend converts them.
protocol EventPoster: AnyObject {
    func movePointer(x: Double, y: Double, window: InputCurrentWindow) throws
    func buttonDown(x: Double, y: Double, window: InputCurrentWindow) throws
    func buttonUp(x: Double, y: Double, window: InputCurrentWindow) throws
    func keyDown(_ keycode: Int) throws
    func keyUp(_ keycode: Int) throws
}

protocol ReceiptSink: AnyObject {
    func writeAttempt(_ record: [String: Any], evidenceDir: String) -> String?
    func writeResult(_ record: [String: Any], evidenceDir: String) -> String?
}

// MARK: - Receipts (G12)

func receiptData(_ record: [String: Any]) -> Data? {
    guard JSONSerialization.isValidJSONObject(record),
        var data = try? JSONSerialization.data(withJSONObject: record, options: [.sortedKeys])
    else { return nil }
    data.append(10)
    return data
}

/// `evidence_dir` must be new: it is created with mode 0700, then `attempt.json` and
/// `result.json` are created exclusively with mode 0600 and synced. Nothing is overwritten.
final class FileReceiptSink: ReceiptSink {
    func writeAttempt(_ record: [String: Any], evidenceDir: String) -> String? {
        guard let data = receiptData(record) else { return nil }
        guard mkdir(evidenceDir, 0o700) == 0 else { return nil }
        let path = evidenceDir + "/attempt.json"
        do { try writeNew(data, path: path) } catch { return nil }
        syncDirectory(evidenceDir)
        return path
    }

    func writeResult(_ record: [String: Any], evidenceDir: String) -> String? {
        guard let data = receiptData(record) else { return nil }
        let path = evidenceDir + "/result.json"
        do { try writeNew(data, path: path) } catch { return nil }
        syncDirectory(evidenceDir)
        return path
    }

    private func syncDirectory(_ path: String) {
        let fd = open(path, O_RDONLY | O_DIRECTORY | O_NOFOLLOW)
        if fd >= 0 {
            _ = fsync(fd)
            close(fd)
        }
    }
}

// MARK: - Gate (port of InputGate in src/foreground_input.rs)

struct InputOutcome: Equatable {
    let id: String
    var status: String
    var gate: String?
    var reason: String?
    var attempted = false
    var releaseAttempted = false
    var releaseConfirmed = false
    var attemptFile: String?
    var resultFile: String?

    static func stopped(_ id: String, _ status: String, _ gate: String, _ reason: String) -> InputOutcome {
        InputOutcome(id: id, status: status, gate: gate, reason: reason)
    }

    /// The `result` of the `action` reply. `action_allowed` stays false, as everywhere.
    var json: [String: Any] {
        [
            "status": status, "action_allowed": false,
            "gate": gate as Any? ?? NSNull(), "reason": reason as Any? ?? NSNull(),
            "attempted": attempted, "release_attempted": releaseAttempted,
            "release_confirmed": releaseConfirmed,
            "attempt_file": attemptFile as Any? ?? NSNull(), "result_file": resultFile as Any? ?? NSNull(),
        ]
    }
}

final class InputGate {
    private(set) var arming: InputArming?
    private(set) var halted = false
    private var executed: [InputRequest] = []
    private var spent: Set<String> = []

    init(arming: InputArming?) { self.arming = arming }

    var remainingEvents: Int? { arming?.remaining }

    /// The same order as `InputGate::check` in the Rust model.
    func check(
        _ req: InputRequest, latest: InputObservation?, current: InputCurrentWindow?,
        live: InputLiveFacts, nowNs: UInt64
    ) -> (gate: String, reason: String)? {
        guard let arming = arming else { return ("G1", "input_disabled") }
        if halted { return ("G13", "halted_after_delivery_unknown") }
        if arming.expired(nowNs) { return ("G1", "arming_expired") }
        if arming.remaining == 0 { return ("G1", "event_limit_reached") }
        if let done = executed.first(where: { $0.id == req.id }) {
            return done == req ? ("G13", "request_already_executed") : ("G13", "request_id_reused")
        }
        if spent.contains(req.sha256) { return ("G5", "observation_already_used") }
        if !arming.bundles.contains(req.bundleId) { return ("G2", "bundle_not_allowed") }
        if case .keyTap(let keycode) = req.event, !arming.keycodes.contains(keycode) {
            return ("G2", "keycode_not_allowed")
        }
        if live.currentBundleId != req.bundleId { return ("G3", "bundle_mismatch") }
        // A window or process that cannot be sampled now is an identity mismatch.
        guard let current = current else { return ("G3", "identity_mismatch") }
        if req.processStart != current.processStart { return ("G3", "process_start_mismatch") }
        guard let latest = latest, latest.sha256 == req.sha256 else { return ("G5", "observation_mismatch") }
        // review_foreground
        if nowNs < latest.capturedAtNs || nowNs - latest.capturedAtNs > maxObservationAgeNs {
            return ("G5", "observation_expired")
        }
        if req.pid != latest.pid || req.windowId != latest.windowId { return ("G3", "target_mismatch") }
        if current.pid != latest.pid || current.windowId != latest.windowId || current.frame != latest.frame
            || current.processStart != latest.processStart
        {
            return ("G3", "identity_mismatch")
        }
        if let p = req.event.point {
            guard p.x >= 0, p.y >= 0, p.x < latest.frame.width, p.y < latest.frame.height else {
                return ("G6", "point_outside_window")
            }
            if !live.pointUnoccluded { return ("G7", "point_occluded") }
        }
        if !live.targetFrontmost { return ("G8", "target_not_frontmost") }
        if !live.focusUnchanged { return ("G8", "focus_changed") }
        if !live.hidIdle { return ("G9", "hid_not_idle") }
        if !live.secureInputOff { return ("G9", "secure_input_enabled") }
        if !live.inputSourceOk { return ("G9", "input_source_unexpected") }
        return nil
    }

    /// Gates, `attempt` receipt, one delivery sequence, post-check, `result` receipt.
    /// Never retries. After `delivery_unknown` the gate halts until a new arming.
    func submit(
        _ req: InputRequest, latest: InputObservation?, current: InputCurrentWindow?,
        live: InputLiveFacts, nowNs: UInt64, sink: ReceiptSink, poster: EventPoster,
        verifyAfter: () -> Bool
    ) -> InputOutcome {
        if let (gate, reason) = check(req, latest: latest, current: current, live: live, nowNs: nowNs) {
            return .stopped(req.id, arming == nil ? "unsupported" : "refused", gate, reason)
        }
        // check() returned nil, so both exist.
        let window = current!
        let attempt: [String: Any] = [
            "schema": inputAttemptSchema, "id": req.id, "delivery": "foreground",
            "event": req.event.json,
            "target": [
                "bundle_id": req.bundleId, "pid": req.pid, "window_id": req.windowId,
                "process_start_unix_seconds": req.processStart,
            ],
            "observation_sha256": req.sha256, "evidence_dir": req.evidenceDir,
            "events_remaining_before": arming?.remaining ?? 0,
            "sampled_window_frame_points": [
                "x": window.frame.x, "y": window.frame.y, "width": window.frame.width,
                "height": window.frame.height,
            ],
        ]
        guard let attemptFile = sink.writeAttempt(attempt, evidenceDir: req.evidenceDir) else {
            return .stopped(req.id, "refused", "G12", "attempt_not_recorded")
        }
        arming?.remaining -= 1
        // From here the request and its observation are spent, whatever happens.
        executed.append(req)
        spent.insert(req.sha256)
        var out = InputOutcome(id: req.id, status: "posted_unverified", attempted: true)
        out.attemptFile = attemptFile
        deliver(req.event, window: window, poster: poster, out: &out)
        if out.status == "posted_unverified" && !verifyAfter() {
            markUnknown(&out, "G3", "post_check_failed")
        }
        var record = out.json
        record["schema"] = inputResultSchema
        record["id"] = req.id
        record.removeValue(forKey: "action_allowed")
        record.removeValue(forKey: "result_file")
        if let resultFile = sink.writeResult(record, evidenceDir: req.evidenceDir) {
            out.resultFile = resultFile
        } else {
            markUnknown(&out, "G12", "result_not_recorded")
        }
        if out.status == "delivery_unknown" { halted = true }
        return out
    }

    private func markUnknown(_ out: inout InputOutcome, _ gate: String, _ reason: String) {
        out.status = "delivery_unknown"
        out.gate = gate
        out.reason = reason
    }

    /// One synchronous press and release. A failure after a press that may have been sent
    /// attempts one release (cleanup, not a retry). A press that certainly was not sent gets
    /// no release, since an unpaired release would itself be a stray event.
    private func deliver(_ event: InputEvent, window: InputCurrentWindow, poster: EventPoster, out: inout InputOutcome) {
        func attempt(_ body: () throws -> Void) -> PostError? {
            do {
                try body()
                return nil
            } catch let error as PostError {
                return error
            } catch {
                return .maybeSent("\(error)")
            }
        }
        let pressed: PostError?
        switch event {
        case .pointerMove(let x, let y): pressed = attempt { try poster.movePointer(x: x, y: y, window: window) }
        case .click(let x, let y): pressed = attempt { try poster.buttonDown(x: x, y: y, window: window) }
        case .keyTap(let keycode): pressed = attempt { try poster.keyDown(keycode) }
        }
        if case .notSent = pressed {
            out.status = "refused"
            out.gate = "G10"
            out.reason = "press_not_sent"
            return
        }
        let released: PostError?
        switch event {
        case .pointerMove:
            if pressed != nil { markUnknown(&out, "G10", "post_failed") }
            return
        case .click(let x, let y): released = attempt { try poster.buttonUp(x: x, y: y, window: window) }
        case .keyTap(let keycode): released = attempt { try poster.keyUp(keycode) }
        }
        out.releaseAttempted = true
        out.releaseConfirmed = released == nil
        if pressed != nil {
            markUnknown(&out, "G10", "post_failed")
        } else if released != nil {
            markUnknown(&out, "G10", "release_failed")
        }
    }
}

// MARK: - Controller: one per ReachAgent process, only with --enable-input

func monotonicNs() -> UInt64 { DispatchTime.now().uptimeNanoseconds }

/// Hash a file the service itself just wrote (no symlink following, bounded size).
func fileSha256(_ path: String) -> String? {
    let fd = open(path, O_RDONLY | O_NOFOLLOW)
    guard fd >= 0 else { return nil }
    defer { close(fd) }
    var hasher = SHA256()
    var total = 0
    var chunk = [UInt8](repeating: 0, count: 65536)
    while true {
        let n = read(fd, &chunk, chunk.count)
        if n < 0 && errno == EINTR { continue }
        if n < 0 { return nil }
        if n == 0 { break }
        total += n
        if total > 256 * 1024 * 1024 { return nil }
        hasher.update(data: chunk[0..<n])
    }
    return hasher.finalize().map { String(format: "%02x", $0) }.joined()
}

final class InputController: @unchecked Sendable {
    let backendName: String
    private let lock = NSLock()
    private let gate: InputGate
    private var latest: InputObservation?
    private let sampler: InputSampler
    private let poster: EventPoster
    private let sink: ReceiptSink
    private let now: () -> UInt64

    init(
        arming: InputArming, backendName: String, sampler: InputSampler, poster: EventPoster,
        sink: ReceiptSink, now: @escaping () -> UInt64
    ) {
        self.gate = InputGate(arming: arming)
        self.backendName = backendName
        self.sampler = sampler
        self.poster = poster
        self.sink = sink
        self.now = now
    }

    /// The `input` block of `describe`.
    func describe() -> [String: Any] {
        lock.lock()
        defer { lock.unlock() }
        guard let arming = gate.arming else { return ["state": "disabled", "backend": backendName] }
        let live = !gate.halted && !arming.expired(now()) && arming.remaining > 0
        return [
            "state": live ? "armed" : "disabled",
            "backend": backendName,
            "allowed_bundle_ids": arming.bundles,
            "allowed_keycodes": arming.keycodes.sorted(),
            "expires_unix_seconds": arming.expiresUnixSeconds,
            "events_remaining": arming.remaining,
            "halted": gate.halted,
            "verified": false,
        ]
    }

    /// Forget the previous observation before a capture runs.
    func captureStarting() {
        lock.lock()
        latest = nil
        lock.unlock()
    }

    /// Record a successful `capture` as the latest observation and return its hash.
    /// The hash is of the file ReachAgent wrote; the process start is sampled here.
    func captureFinished(pid: Int32, windowId: UInt32, report: [String: Any]) -> String? {
        guard let path = report["image_path"] as? String, let sha = fileSha256(path),
            let f = report["window_frame_points"] as? [String: Any],
            let x = plainNumber(f["x"]), let y = plainNumber(f["y"]),
            let w = plainNumber(f["width"]), let h = plainNumber(f["height"]),
            let start = sampler.processStart(pid: pid)
        else { return nil }
        lock.lock()
        defer { lock.unlock() }
        latest = InputObservation(
            sha256: sha, pid: pid, windowId: windowId, frame: InputFrame(x: x, y: y, width: w, height: h),
            processStart: start, capturedAtNs: now())
        return sha
    }

    /// One `action`, start to end under the lock: one event at a time.
    func action(_ request: InputRequest) -> InputOutcome {
        lock.lock()
        defer { lock.unlock() }
        let current = sampler.currentWindow(pid: request.pid, windowId: request.windowId)
        let live = sampler.liveFacts(request, window: current)
        return gate.submit(
            request, latest: latest, current: current, live: live, nowNs: now(), sink: sink,
            poster: poster, verifyAfter: { sampler.verifyAfter(request) })
    }
}

/// Handle `action`: parse, check `post_event`, then the gate. Schema problems and gate
/// refusals are `ok` replies with a status; a missing permission is an error line.
func handleAction(
    _ id: String, _ object: [String: Any], provider: PermissionProvider, input: InputController
) -> String {
    switch parseActionParams(id: id, object["params"]) {
    case .failure(let rejection):
        var outcome = InputOutcome.stopped(id, rejection.status, rejection.gate, rejection.reason)
        outcome.attempted = false
        return okLine(id, outcome.json)
    case .success(let request):
        do {
            try requirePermission("post_event", in: provider.current())
        } catch let error as AgentError {
            return errorLine(id, error)
        } catch {
            return errorLine(id, AgentError("permission_denied", "\(error)"))
        }
        return okLine(id, input.action(request).json)
    }
}

// MARK: - Fake backend (--self-test and --test-mode). It records events in memory only.

final class FakeInputBackend: InputSampler, EventPoster, @unchecked Sendable {
    private let lock = NSLock()
    var processStarts: [Int32: Double] = [:]
    var windows: [String: InputFrame] = [:]
    var bundles: [Int32: String] = [:]
    var facts = InputLiveFacts(
        currentBundleId: nil, targetFrontmost: true, focusUnchanged: true, hidIdle: true,
        secureInputOff: true, inputSourceOk: true, pointUnoccluded: true)
    var verifyResult = true
    /// Failures to return, by call name ("down", "up", "move", "key_down", "key_up").
    var failures: [String: PostError] = [:]
    private(set) var events: [String] = []

    func processStart(pid: Int32) -> Double? { processStarts[pid] }

    func currentWindow(pid: Int32, windowId: UInt32) -> InputCurrentWindow? {
        guard let frame = windows["\(pid)/\(windowId)"], let start = processStarts[pid] else { return nil }
        return InputCurrentWindow(pid: pid, windowId: windowId, frame: frame, processStart: start)
    }

    func liveFacts(_ request: InputRequest, window: InputCurrentWindow?) -> InputLiveFacts {
        var f = facts
        f.currentBundleId = bundles[request.pid]
        return f
    }

    func verifyAfter(_ request: InputRequest) -> Bool { verifyResult }

    private func record(_ name: String, _ text: String) throws {
        lock.lock()
        defer { lock.unlock() }
        events.append(text)
        if let failure = failures[name] { throw failure }
    }

    func movePointer(x: Double, y: Double, window: InputCurrentWindow) throws { try record("move", "move \(x) \(y)") }
    func buttonDown(x: Double, y: Double, window: InputCurrentWindow) throws { try record("down", "down \(x) \(y)") }
    func buttonUp(x: Double, y: Double, window: InputCurrentWindow) throws { try record("up", "up \(x) \(y)") }
    func keyDown(_ keycode: Int) throws { try record("key_down", "key_down \(keycode)") }
    func keyUp(_ keycode: Int) throws { try record("key_up", "key_up \(keycode)") }
}

/// The controller for `--test-mode --enable-input` and the self-test. The fake knows one
/// target: pid 42, window 7, the 2x2 frame of FakeCaptureBackend, the fixture bundle id.
/// Receipts are real files, so the evidence directory behaves as with the system backend.
func makeFakeInputController(arming: InputArming, now: @escaping () -> UInt64) -> (
    controller: InputController, fake: FakeInputBackend
) {
    let fake = FakeInputBackend()
    fake.processStarts = [42: 1000.5]
    fake.windows = ["42/7": InputFrame(x: 0, y: 0, width: 2, height: 2)]
    fake.bundles = [42: fixtureBundleId]
    let controller = InputController(
        arming: arming, backendName: "fake", sampler: fake, poster: fake, sink: FileReceiptSink(), now: now)
    return (controller, fake)
}

final class MemoryReceiptSink: ReceiptSink {
    var failAttempt = false
    var failResult = false
    private(set) var attempts: [[String: Any]] = []
    private(set) var results: [[String: Any]] = []
    func writeAttempt(_ record: [String: Any], evidenceDir: String) -> String? {
        if failAttempt { return nil }
        attempts.append(record)
        return evidenceDir + "/attempt.json"
    }
    func writeResult(_ record: [String: Any], evidenceDir: String) -> String? {
        if failResult { return nil }
        results.append(record)
        return evidenceDir + "/result.json"
    }
}

// MARK: - Self-check for --self-test: fake backend and fake clock only

let fixtureBundleId = "org.local.rust-computer-use.target-event-fixture"

func inputSelfCheck() -> [String] {
    var failures: [String] = []
    func check(_ condition: Bool, _ what: String) { if !condition { failures.append(what) } }

    // Start-up options.
    let base = ["--serve", "--socket", "/tmp/x"]
    let full = base + [
        "--enable-input", "--allow-bundle", fixtureBundleId, "--allow-keycode", "18",
        "--input-ttl-seconds", "60", "--max-input-events", "3",
    ]
    check((try? parseServeArguments(base)) == ServeConfig(socket: "/tmp/x", testMode: false, input: nil),
        "plain --serve parses without input")
    check((try? parseServeArguments(base + ["--test-mode"]))?.input == nil, "--test-mode alone has no input")
    let parsed = try? parseServeArguments(full)
    check(parsed?.input == InputOptions(bundles: [fixtureBundleId], keycodes: [18], ttlSeconds: 60, maxEvents: 3),
        "--enable-input parses its options")
    for bad in [
        base + ["--allow-bundle", fixtureBundleId],
        base + ["--input-ttl-seconds", "60"],
        base + ["--enable-input", "--input-ttl-seconds", "60", "--max-input-events", "3"],
        base + ["--enable-input", "--allow-bundle", fixtureBundleId, "--max-input-events", "3"],
        base + ["--enable-input", "--allow-bundle", fixtureBundleId, "--input-ttl-seconds", "60"],
        base + ["--enable-input", "--allow-bundle", "com.google.Chrome", "--input-ttl-seconds", "60", "--max-input-events", "3"],
        base + ["--enable-input", "--allow-bundle", "*", "--input-ttl-seconds", "60", "--max-input-events", "3"],
        base + ["--enable-input", "--allow-bundle", fixtureBundleId, "--allow-bundle", fixtureBundleId, "--input-ttl-seconds", "60", "--max-input-events", "3"],
        base + ["--enable-input", "--allow-bundle", fixtureBundleId, "--input-ttl-seconds", "601", "--max-input-events", "3"],
        base + ["--enable-input", "--allow-bundle", fixtureBundleId, "--input-ttl-seconds", "0", "--max-input-events", "3"],
        base + ["--enable-input", "--allow-bundle", fixtureBundleId, "--input-ttl-seconds", "60", "--max-input-events", "21"],
        base + ["--enable-input", "--allow-bundle", fixtureBundleId, "--input-ttl-seconds", "60", "--max-input-events", "0"],
        base + ["--enable-input", "--allow-bundle", fixtureBundleId, "--allow-keycode", "128", "--input-ttl-seconds", "60", "--max-input-events", "3"],
        base + ["--enable-input", "--allow-bundle", fixtureBundleId, "--allow-keycode", "-1", "--input-ttl-seconds", "60", "--max-input-events", "3"],
        base + ["--enable-input", "--enable-input", "--allow-bundle", fixtureBundleId, "--input-ttl-seconds", "60", "--max-input-events", "3"],
        base + ["--enable-input", "--allow-bundle", fixtureBundleId, "--input-ttl-seconds", "60", "--max-input-events", "3", "--allow-modifier", "cmd"],
        base + ["--allow-bundle"],
        base + ["--frobnicate"],
    ] {
        check((try? parseServeArguments(bad)) == nil, "usage error for \(bad.dropFirst(3).joined(separator: " "))")
    }

    // Arming limits.
    let options = InputOptions(bundles: [fixtureBundleId], keycodes: [18], ttlSeconds: 60, maxEvents: 3)
    check(InputArming(options, nowNs: 1, nowUnixSeconds: 1) != nil, "a valid arming is accepted")
    var o = options
    o.bundles = ["com.google.Chrome"]
    check(InputArming(o, nowNs: 1, nowUnixSeconds: 1) == nil, "Chrome is not approved")
    o = options
    o.bundles = []
    check(InputArming(o, nowNs: 1, nowUnixSeconds: 1) == nil, "an empty bundle list is refused")
    o = options
    o.ttlSeconds = 601
    check(InputArming(o, nowNs: 1, nowUnixSeconds: 1) == nil, "ttl above 600 s is refused")
    o = options
    o.maxEvents = 21
    check(InputArming(o, nowNs: 1, nowUnixSeconds: 1) == nil, "more than 20 events is refused")
    o = options
    o.keycodes = [128]
    check(InputArming(o, nowNs: 1, nowUnixSeconds: 1) == nil, "keycode 128 is refused")

    // Request parsing.
    let sha = String(repeating: "a", count: 64)
    let target: [String: Any] = [
        "bundle_id": fixtureBundleId, "pid": 42, "window_id": 7, "process_start_unix_seconds": 1000.5,
    ]
    func params(_ event: [String: Any], sha: String = sha, dir: String = "/tmp/e") -> [String: Any] {
        ["delivery": "foreground", "target": target, "observation": ["image_sha256": sha], "event": event, "evidence_dir": dir]
    }
    func parse(_ p: Any?, id: String = "r") -> Result<InputRequest, InputRejection> { parseActionParams(id: id, p) }
    func rejection(_ r: Result<InputRequest, InputRejection>) -> InputRejection? {
        if case .failure(let e) = r { return e }
        return nil
    }
    if case .success(let r) = parse(params(["kind": "click", "button": "left", "x": 1, "y": 1.5])) {
        check(r.event == .click(x: 1, y: 1.5) && r.pid == 42 && r.windowId == 7 && r.processStart == 1000.5,
            "a click request parses")
    } else {
        failures.append("a valid click request is refused")
    }
    check(rejection(parse(params(["kind": "key_tap", "keycode": 18]))) == nil, "a key_tap request parses")
    check(rejection(parse(params(["kind": "pointer_move", "x": 0, "y": 0]))) == nil, "a pointer_move request parses")
    var bg = params(["kind": "key_tap", "keycode": 18])
    bg["delivery"] = "background"
    check(rejection(parse(bg)) == InputRejection(status: "unsupported", gate: "delivery", reason: "background delivery is not supported"),
        "background delivery is unsupported")
    var extra = params(["kind": "key_tap", "keycode": 18])
    extra["helper"] = "/x"
    for (p, what) in [
        (extra, "an unknown top-level field"),
        (params(["kind": "key_tap", "keycode": 18, "modifiers": ["cmd"]]), "a modifier field"),
        (params(["kind": "key_tap", "keycode": 128]), "keycode 128"),
        (params(["kind": "click", "button": "right", "x": 1, "y": 1]), "the right button"),
        (params(["kind": "scroll", "dy": 1]), "a scroll event"),
        (params(["kind": "drag", "x": 1, "y": 1]), "a drag event"),
        (params(["kind": "activate_app"]), "app activation"),
        (params(["kind": "click", "button": "left", "x": true, "y": 1]), "a boolean coordinate"),
        (params(["kind": "key_tap", "keycode": 18], sha: "A" + String(sha.dropFirst())), "an uppercase hash"),
        (params(["kind": "key_tap", "keycode": 18], dir: "relative"), "a relative evidence_dir"),
    ] as [([String: Any], String)] {
        check(rejection(parse(p))?.gate == "schema", "schema refuses \(what)")
    }
    check(rejection(parse(params(["kind": "key_tap", "keycode": 18]), id: ""))?.gate == "schema", "an empty id is refused")

    // Gate cases: fake backend, fake clock, in-memory receipts.
    let fake = FakeInputBackend()
    fake.processStarts = [42: 1000.5]
    fake.windows = ["42/7": InputFrame(x: 0, y: 0, width: 100, height: 50)]
    fake.bundles = [42: fixtureBundleId]
    let observation = InputObservation(
        sha256: sha, pid: 42, windowId: 7, frame: InputFrame(x: 0, y: 0, width: 100, height: 50),
        processStart: 1000.5, capturedAtNs: 1_000)
    let current = fake.currentWindow(pid: 42, windowId: 7)
    func request(_ id: String, _ event: InputEvent, sha: String = sha, bundle: String = fixtureBundleId,
                 window: UInt32 = 7, start: Double = 1000.5) -> InputRequest {
        InputRequest(id: id, bundleId: bundle, pid: 42, windowId: window, processStart: start, sha256: sha,
                     event: event, evidenceDir: "/tmp/e")
    }
    func run(_ gate: InputGate, _ req: InputRequest, latest: InputObservation? = observation,
             current: InputCurrentWindow? = current, facts: InputLiveFacts? = nil, now: UInt64 = 2_000,
             sink: MemoryReceiptSink = MemoryReceiptSink()) -> InputOutcome {
        var live = facts ?? fake.liveFacts(req, window: current)
        if facts != nil, live.currentBundleId == nil { live.currentBundleId = fixtureBundleId }
        return gate.submit(req, latest: latest, current: current, live: live, nowNs: now, sink: sink,
                           poster: fake, verifyAfter: { fake.verifyAfter(req) })
    }
    func armed(_ events: Int = 3, ttl: Int = 60) -> InputGate {
        var o = options
        o.maxEvents = events
        o.ttlSeconds = ttl
        return InputGate(arming: InputArming(o, nowNs: 1_000, nowUnixSeconds: 0)!)
    }
    func reason(_ out: InputOutcome) -> String { "\(out.status)/\(out.gate ?? "-")/\(out.reason ?? "-")" }
    let click = InputEvent.click(x: 10, y: 10)

    check(reason(run(InputGate(arming: nil), request("a", click))) == "unsupported/G1/input_disabled",
        "no arming is input_disabled")
    check(reason(run(armed(ttl: 1), request("a", click), now: 1_000 + 2_000_000_000)) == "refused/G1/arming_expired",
        "an expired arming refuses")
    check(reason(run(armed(), request("a", click), now: 999)) == "refused/G1/arming_expired",
        "a clock before the arming refuses")

    let limit = armed(1)
    let first = run(limit, request("a", click))
    check(reason(first) == "posted_unverified/-/-" && first.attempted && first.releaseAttempted && first.releaseConfirmed,
        "a click passes every gate with the fake backend")
    check(fake.events == ["down 10.0 10.0", "up 10.0 10.0"], "the fake backend saw one press and one release")
    check(reason(run(limit, request("b", click, sha: String(repeating: "b", count: 64)))) == "refused/G1/event_limit_reached",
        "the event limit refuses")

    let replay = armed()
    _ = run(replay, request("a", click))
    check(reason(run(replay, request("a", click))) == "refused/G13/request_already_executed", "a replay is refused")
    check(reason(run(replay, request("a", .click(x: 11, y: 10)))) == "refused/G13/request_id_reused", "a reused id is refused")
    check(reason(run(replay, request("b", click))) == "refused/G5/observation_already_used", "a spent observation is refused")
    check(replay.remainingEvents == 2, "refusals do not use up events")

    let fresh = armed()
    let before = fake.events.count
    let safari = request("a", click, bundle: "com.apple.Safari")
    check(reason(run(fresh, safari)) == "refused/G2/bundle_not_allowed", "a bundle outside the arming is refused")
    check(reason(run(fresh, request("a", .keyTap(keycode: 49)))) == "refused/G2/keycode_not_allowed", "a keycode outside the arming is refused")
    var facts = fake.facts
    facts.currentBundleId = "com.apple.Safari"
    check(reason(run(fresh, request("a", click), facts: facts)) == "refused/G3/bundle_mismatch", "a bundle mismatch is refused")
    check(reason(run(fresh, request("a", click), current: nil)) == "refused/G3/identity_mismatch", "a missing window is refused")
    check(reason(run(fresh, request("a", click, start: 1000.25))) == "refused/G3/process_start_mismatch", "a process start mismatch is refused")
    check(reason(run(fresh, request("a", click, sha: String(repeating: "c", count: 64)))) == "refused/G5/observation_mismatch",
        "a hash that is not the latest capture is refused")
    check(reason(run(fresh, request("a", click), latest: nil)) == "refused/G5/observation_mismatch", "no capture at all is refused")
    check(reason(run(fresh, request("a", click), now: 1_000 + maxObservationAgeNs + 1)) == "refused/G5/observation_expired",
        "an old observation is refused")
    let otherWindow = InputCurrentWindow(pid: 42, windowId: 8, frame: observation.frame, processStart: 1000.5)
    check(reason(run(fresh, request("a", click, window: 8), current: otherWindow)) == "refused/G3/target_mismatch",
        "a target that is not the captured window is refused")
    let moved = InputCurrentWindow(pid: 42, windowId: 7, frame: InputFrame(x: 5, y: 0, width: 100, height: 50), processStart: 1000.5)
    check(reason(run(fresh, request("a", click), current: moved)) == "refused/G3/identity_mismatch", "a moved window is refused")
    check(reason(run(fresh, request("a", .click(x: 100, y: 10)))) == "refused/G6/point_outside_window", "a point outside the window is refused")
    check(reason(run(fresh, request("a", .pointerMove(x: -1, y: 10)))) == "refused/G6/point_outside_window", "a negative point is refused")
    facts = fake.facts
    facts.pointUnoccluded = false
    check(reason(run(fresh, request("a", click), facts: facts)) == "refused/G7/point_occluded", "an occluded point is refused")
    facts = fake.facts
    facts.targetFrontmost = false
    check(reason(run(fresh, request("a", click), facts: facts)) == "refused/G8/target_not_frontmost", "a background target is refused")
    facts = fake.facts
    facts.focusUnchanged = false
    check(reason(run(fresh, request("a", click), facts: facts)) == "refused/G8/focus_changed", "a focus change is refused")
    facts = fake.facts
    facts.hidIdle = false
    check(reason(run(fresh, request("a", click), facts: facts)) == "refused/G9/hid_not_idle", "held input is refused")
    facts = fake.facts
    facts.secureInputOff = false
    check(reason(run(fresh, request("a", click), facts: facts)) == "refused/G9/secure_input_enabled", "secure input is refused")
    facts = fake.facts
    facts.inputSourceOk = false
    check(reason(run(fresh, request("a", click), facts: facts)) == "refused/G9/input_source_unexpected", "an unexpected input source is refused")
    check(reason(run(fresh, request("a", .keyTap(keycode: 18)), facts: { var f = fake.facts; f.pointUnoccluded = false; return f }()))
        == "posted_unverified/-/-", "occlusion does not apply to key_tap")
    check(fake.events.count == before + 2, "no refusal sent an event")
    check(InputLiveFacts.failClosed.targetFrontmost == false && InputLiveFacts.failClosed.hidIdle == false,
        "unknown facts fail closed")

    // Receipts and no-retry.
    let receipts = armed()
    let failingSink = MemoryReceiptSink()
    failingSink.failAttempt = true
    let count = fake.events.count
    check(reason(run(receipts, request("a", click), sink: failingSink)) == "refused/G12/attempt_not_recorded",
        "no attempt receipt means no delivery")
    check(fake.events.count == count && receipts.remainingEvents == 3, "nothing was sent or spent without an attempt receipt")
    let okSink = MemoryReceiptSink()
    let retried = run(receipts, request("a", click), sink: okSink)
    check(reason(retried) == "posted_unverified/-/-", "the same request may run once the attempt is recorded")
    check(okSink.attempts.first?["schema"] as? String == inputAttemptSchema
        && okSink.results.first?["schema"] as? String == inputResultSchema
        && okSink.results.first?["status"] as? String == "posted_unverified",
        "attempt and result receipts are written")
    check(okSink.attempts.first?["events_remaining_before"] as? Int == 3, "the attempt records the count before")

    fake.failures = ["down": .notSent("test")]
    let notSent = armed()
    let n0 = fake.events.count
    let ns = run(notSent, request("a", click))
    check(reason(ns) == "refused/G10/press_not_sent" && !ns.releaseAttempted, "a press that was not sent gets no release")
    check(fake.events.count == n0 + 1 && notSent.remainingEvents == 2 && !notSent.halted,
        "press_not_sent spends the event and does not halt")

    fake.failures = ["down": .maybeSent("test")]
    let unknown = armed()
    let u = run(unknown, request("a", click))
    check(reason(u) == "delivery_unknown/G10/post_failed" && u.releaseAttempted && u.releaseConfirmed,
        "a failed press attempts one release")
    check(unknown.halted, "delivery_unknown halts the gate")
    fake.failures = [:]
    check(reason(run(unknown, request("b", click, sha: String(repeating: "b", count: 64)))) == "refused/G13/halted_after_delivery_unknown",
        "a halted gate refuses every later request")

    fake.failures = ["key_up": .maybeSent("test")]
    let release = armed()
    let r = run(release, request("a", .keyTap(keycode: 18)))
    check(reason(r) == "delivery_unknown/G10/release_failed" && r.releaseAttempted && !r.releaseConfirmed,
        "a failed release is delivery_unknown")
    fake.failures = ["move": .maybeSent("test")]
    check(reason(run(armed(), request("a", .pointerMove(x: 1, y: 1)))) == "delivery_unknown/G10/post_failed",
        "a failed move is delivery_unknown")
    fake.failures = [:]

    fake.verifyResult = false
    let verify = armed()
    check(reason(run(verify, request("a", click))) == "delivery_unknown/G3/post_check_failed", "a failed post-check is delivery_unknown")
    check(verify.halted, "a failed post-check halts")
    fake.verifyResult = true

    let resultSink = MemoryReceiptSink()
    resultSink.failResult = true
    let noResult = armed()
    check(reason(run(noResult, request("a", click), sink: resultSink)) == "delivery_unknown/G12/result_not_recorded",
        "a missing result receipt is delivery_unknown")
    check(noResult.halted, "a missing result receipt halts")

    let json = InputOutcome.stopped("a", "refused", "G2", "bundle_not_allowed").json
    check(json["action_allowed"] as? Bool == false, "an outcome never allows actions")

    return failures
}
