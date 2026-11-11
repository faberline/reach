import ApplicationServices
import CoreGraphics
import Foundation

// ReachAgent service (faberline/reach-old#33, faberline/reach-old#35 and faberline/reach-old#39). It listens on a Unix domain socket and speaks the
// line-based JSON protocol described in docs/AGENT-SERVICE.md. Window capture lives in
// ReachAgentCapture.swift and runs only when `screen_recording` is granted. Accessibility
// snapshots live in ReachAgentAccessibility.swift and run only when `accessibility` is granted.
// The service asks for no permission at start-up. It posts no input event unless it was
// started with --enable-input (ReachAgentInput.swift); only ReachAgentInputSystem.swift may
// deliver an event, and that backend is unreachable in --test-mode, under CI and by default.

let protocolVersion = 1
let appVersion = "0.1.0"
let methods = ["describe", "permissions", "request_permissions", "capture", "capture_all_windows", "accessibility", "accessibility_subtree"]
let permissionNames = ["accessibility", "screen_recording", "post_event"]
let maxLineBytes = 65536
let maxSocketPathBytes = 100
let maxImagePathBytes = 1024
let captureTimeoutSeconds = 30.0

struct AgentError: Error {
    let code: String
    let message: String
    var permission: String? = nil
    var state: String? = nil
    init(_ code: String, _ message: String) {
        self.code = code
        self.message = message
    }
}

// MARK: - Permissions

protocol PermissionProvider {
    /// True only for the provider that may ask the system for a permission.
    var performsRequests: Bool { get }
    /// Pre-flight only; never shows a prompt.
    func current() -> [String: String]
    func request(_ names: [String]) -> [String: String]
}

struct SystemPermissionProvider: PermissionProvider {
    let performsRequests = true
    /// Present only when start-up authorized the system input backend.
    var inputLaunch: SystemInputLaunch? = nil

    func current() -> [String: String] {
        func state(_ granted: Bool) -> String { granted ? "granted" : "denied" }
        return [
            "accessibility": state(AXIsProcessTrusted()),
            "screen_recording": state(CGPreflightScreenCaptureAccess()),
            "post_event": state(CGPreflightPostEventAccess()),
        ]
    }

    /// Only called by an explicit `request_permissions`. `post_event` reaches this only while
    /// input is enabled (`parseRequested`), and is asked for only with the system input backend.
    func request(_ names: [String]) -> [String: String] {
        if names.contains("post_event"), let launch = inputLaunch {
            requestPostEventAccess(launch)
        }
        if names.contains("accessibility") {
            let options = ["AXTrustedCheckOptionPrompt": true] as CFDictionary
            _ = AXIsProcessTrustedWithOptions(options)
        }
        if names.contains("screen_recording") {
            _ = CGRequestScreenCaptureAccess()
        }
        return current()
    }
}

/// Used by --self-test and --test-mode: no system permission API is called.
struct TestPermissionProvider: PermissionProvider {
    let performsRequests = false
    func current() -> [String: String] {
        ["accessibility": "unknown", "screen_recording": "unknown", "post_event": "unknown"]
    }
    func request(_ names: [String]) -> [String: String] { current() }
}

/// Used by --self-test: `screen_recording` and `accessibility` can be changed between
/// connections, and nothing calls a system permission API.
final class SelfTestPermissionProvider: PermissionProvider, @unchecked Sendable {
    let performsRequests = false
    private let lock = NSLock()
    private var screenRecordingState = "unknown"
    private var accessibilityState = "unknown"
    private var postEventState = "unknown"
    var screenRecording: String {
        get { lock.lock(); defer { lock.unlock() }; return screenRecordingState }
        set { lock.lock(); screenRecordingState = newValue; lock.unlock() }
    }
    var accessibility: String {
        get { lock.lock(); defer { lock.unlock() }; return accessibilityState }
        set { lock.lock(); accessibilityState = newValue; lock.unlock() }
    }
    var postEvent: String {
        get { lock.lock(); defer { lock.unlock() }; return postEventState }
        set { lock.lock(); postEventState = newValue; lock.unlock() }
    }
    func current() -> [String: String] {
        ["accessibility": accessibility, "screen_recording": screenRecording, "post_event": postEvent]
    }
    func request(_ names: [String]) -> [String: String] { current() }
}

/// A method that needs a permission runs only when it is granted. `capture` needs
/// `screen_recording`; `accessibility` and `accessibility_subtree` need `accessibility`.
func requirePermission(_ name: String, in set: [String: String]) throws {
    let state = set[name] ?? "unknown"
    if state != "granted" {
        var error = AgentError("permission_denied", "\(name) is \(state)")
        error.permission = name
        error.state = state
        throw error
    }
}

// MARK: - Protocol

func jsonLine(_ object: [String: Any]) -> String {
    let data = try! JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
    return String(data: data, encoding: .utf8)!
}

func okLine(_ id: String, _ result: [String: Any]) -> String {
    jsonLine(["version": protocolVersion, "id": id, "ok": true, "result": result])
}

func errorLine(_ id: String?, _ error: AgentError) -> String {
    var body: [String: Any] = ["code": error.code, "message": error.message]
    if let permission = error.permission { body["permission"] = permission }
    if let state = error.state { body["state"] = state }
    return jsonLine([
        "version": protocolVersion, "id": id as Any? ?? NSNull(), "ok": false, "error": body,
    ])
}

/// True when any JSON object in the text names a key twice (JSONSerialization hides this).
func hasDuplicateKeys(_ text: String) -> Bool {
    struct Frame {
        var isObject: Bool
        var keys: Set<String> = []
        var expectKey: Bool
    }
    var stack: [Frame] = []
    let chars = Array(text.unicodeScalars)
    var i = 0
    while i < chars.count {
        let c = chars[i]
        switch c {
        case "{":
            stack.append(Frame(isObject: true, expectKey: true))
            i += 1
        case "[":
            stack.append(Frame(isObject: false, expectKey: false))
            i += 1
        case "}", "]":
            if !stack.isEmpty { stack.removeLast() }
            i += 1
        case ",":
            if var top = stack.popLast() {
                if top.isObject { top.expectKey = true }
                stack.append(top)
            }
            i += 1
        case "\"":
            var raw = ""
            var j = i + 1
            while j < chars.count && chars[j] != "\"" {
                if chars[j] == "\\" {
                    raw.unicodeScalars.append(chars[j])
                    j += 1
                }
                if j < chars.count { raw.unicodeScalars.append(chars[j]) }
                j += 1
            }
            i = j + 1
            if var top = stack.popLast() {
                if top.isObject && top.expectKey {
                    let decoded =
                        (try? JSONSerialization.jsonObject(
                            with: Data(("\"" + raw + "\"").utf8), options: [.fragmentsAllowed])
                            as? String) ?? raw
                    if !top.keys.insert(decoded).inserted { return true }
                    top.expectKey = false
                }
                stack.append(top)
            }
        default:
            i += 1
        }
    }
    return false
}

func isPlainInteger(_ value: Any?) -> Int? {
    guard let number = value as? NSNumber, CFGetTypeID(number) != CFBooleanGetTypeID() else {
        return nil
    }
    let type = String(cString: number.objCType)
    if type == "d" || type == "f" { return nil }
    return number.intValue >= 0 ? number.intValue : nil
}

func parseRequested(_ params: Any?, inputEnabled: Bool = false) throws -> [String] {
    let list = (params as? [String: Any])?["permissions"] as? [Any]
    guard let list = list else {
        throw AgentError("invalid_params", "`permissions` must be an array")
    }
    if list.isEmpty { throw AgentError("invalid_params", "`permissions` must not be empty") }
    var requested: [String] = []
    for item in list {
        guard let name = item as? String else {
            throw AgentError("invalid_params", "every permission name must be a string")
        }
        guard permissionNames.contains(name) else {
            throw AgentError("invalid_params", "unknown permission name")
        }
        if requested.contains(name) {
            throw AgentError("invalid_params", "a permission name appears twice")
        }
        requested.append(name)
    }
    if requested.contains("post_event") && !inputEnabled {
        throw AgentError("not_requestable", "post_event cannot be requested: input stays disabled")
    }
    return requested
}

/// `pid`, `window_id` and `image`, nothing else. The Rust `parse_capture_params` has the same rules.
func parseCaptureParams(_ params: Any?, includeOffscreen: Bool) throws -> CaptureRequest {
    guard let object = params as? [String: Any] else {
        throw AgentError("invalid_params", "params must be an object")
    }
    if let extra = object.keys.first(where: { !["pid", "window_id", "image"].contains($0) }) {
        throw AgentError("invalid_params", "unknown parameter `\(extra)`")
    }
    guard let pid = isPlainInteger(object["pid"]), (1...2147483647).contains(pid) else {
        throw AgentError("invalid_params", "`pid` must be a positive 32-bit integer")
    }
    guard let windowId = isPlainInteger(object["window_id"]), (1...4294967295).contains(windowId) else {
        throw AgentError("invalid_params", "`window_id` must be a positive 32-bit integer")
    }
    guard let image = object["image"] as? String else {
        throw AgentError("invalid_params", "`image` must be a string")
    }
    guard image.hasPrefix("/"), image.hasSuffix(".png"), image.utf8.count <= maxImagePathBytes,
        !image.utf8.contains(0)
    else {
        throw AgentError("invalid_params", "`image` must be an absolute .png path of at most 1024 bytes")
    }
    return CaptureRequest(
        windowId: UInt32(windowId), pid: Int32(pid), output: image, includeOffscreen: includeOffscreen)
}

/// Validate, check the permission, then run the backend. A backend never runs without
/// `screen_recording` being granted, and nothing is created before that.
func handleCapture(
    _ id: String, _ object: [String: Any], includeOffscreen: Bool, provider: PermissionProvider,
    backend: CaptureBackend, input: InputController? = nil
) -> String {
    do {
        let request = try parseCaptureParams(object["params"], includeOffscreen: includeOffscreen)
        try requirePermission("screen_recording", in: provider.current())
        // With input enabled, only a single-window `capture` can become the observation that
        // an `action` names. Any capture first forgets the previous one.
        input?.captureStarting()
        switch runBlocking(timeoutSeconds: captureTimeoutSeconds, { try await backend.capture(request) }) {
        case .success(var report):
            if let input = input, !includeOffscreen,
                let sha = input.captureFinished(pid: request.pid, windowId: request.windowId, report: report)
            {
                report["image_sha256"] = sha
            }
            return okLine(id, report)
        case .failure(let failure):
            return errorLine(id, AgentError("capture_failed", failure.description))
        }
    } catch let error as AgentError {
        return errorLine(id, error)
    } catch {
        return errorLine(id, AgentError("capture_failed", "\(error)"))
    }
}

func isPlainBool(_ value: Any?) -> Bool? {
    guard let number = value as? NSNumber, CFGetTypeID(number) == CFBooleanGetTypeID() else {
        return nil
    }
    return number.boolValue
}

func parsePid(_ object: [String: Any]) throws -> Int32 {
    guard let pid = isPlainInteger(object["pid"]), (1...2147483647).contains(pid) else {
        throw AgentError("invalid_params", "`pid` must be a positive 32-bit integer")
    }
    return Int32(pid)
}

/// `pid` and an optional boolean `windows_only`, nothing else. The Rust
/// `parse_accessibility_params` has the same rules.
func parseAccessibilityParams(_ params: Any?) throws -> (pid: Int32, windowsOnly: Bool) {
    guard let object = params as? [String: Any] else {
        throw AgentError("invalid_params", "params must be an object")
    }
    if let extra = object.keys.first(where: { !["pid", "windows_only"].contains($0) }) {
        throw AgentError("invalid_params", "unknown parameter `\(extra)`")
    }
    let pid = try parsePid(object)
    var windowsOnly = false
    if let raw = object["windows_only"] {
        guard let flag = isPlainBool(raw) else {
            throw AgentError("invalid_params", "`windows_only` must be a boolean")
        }
        windowsOnly = flag
    }
    return (pid, windowsOnly)
}

/// `pid`, `source` and `path`, nothing else. The Rust `parse_subtree_params` has the same rules.
func parseSubtreeParams(_ params: Any?) throws -> (pid: Int32, source: String, path: [Int]) {
    guard let object = params as? [String: Any] else {
        throw AgentError("invalid_params", "params must be an object")
    }
    if let extra = object.keys.first(where: { !["pid", "source", "path"].contains($0) }) {
        throw AgentError("invalid_params", "unknown parameter `\(extra)`")
    }
    let pid = try parsePid(object)
    guard let source = object["source"] as? String, ["main", "focused", "windows"].contains(source)
    else {
        throw AgentError("invalid_params", "`source` must be main, focused or windows")
    }
    guard let items = object["path"] as? [Any], items.count <= 8 else {
        throw AgentError("invalid_params", "`path` must be an array of at most 8 integers")
    }
    var path: [Int] = []
    for item in items {
        guard let index = isPlainInteger(item), index < 160 else {
            throw AgentError("invalid_params", "`path` items must be integers from 0 to 159")
        }
        path.append(index)
    }
    if source == "windows" && !(path.first.map { $0 < 32 } ?? false) {
        throw AgentError(
            "invalid_params", "`path` for source windows must start with a window index from 0 to 31")
    }
    return (pid, source, path)
}

/// Validate, check the permission, then run the backend. A backend never runs without
/// `accessibility` being granted.
func handleAccessibility(
    _ id: String, _ object: [String: Any], subtree: Bool, provider: PermissionProvider,
    backend: AccessibilityBackend
) -> String {
    do {
        let report: [String: Any]
        if subtree {
            let request = try parseSubtreeParams(object["params"])
            try requirePermission("accessibility", in: provider.current())
            report = try backend.subtree(pid: request.pid, source: request.source, path: request.path)
        } else {
            let request = try parseAccessibilityParams(object["params"])
            try requirePermission("accessibility", in: provider.current())
            report = try backend.snapshot(pid: request.pid, windowsOnly: request.windowsOnly)
        }
        return okLine(id, report)
    } catch let error as AgentError {
        return errorLine(id, error)
    } catch let failure as AccessibilityFailure {
        return errorLine(id, AgentError("ax_failed", failure.description))
    } catch {
        return errorLine(id, AgentError("ax_failed", "\(error)"))
    }
}

/// One request line (without its newline) in, one response line out.
func handleLine(
    _ line: String, provider: PermissionProvider, backend: CaptureBackend,
    axBackend: AccessibilityBackend, testMode: Bool, input: InputController? = nil
) -> String {
    if line.utf8.count > maxLineBytes {
        return errorLine(nil, AgentError("invalid_json", "request line is too long"))
    }
    if line.utf8.contains(10) || line.utf8.contains(13) {
        return errorLine(nil, AgentError("invalid_json", "a request must be exactly one line"))
    }
    guard let parsed = try? JSONSerialization.jsonObject(with: Data(line.utf8)),
        let object = parsed as? [String: Any], !hasDuplicateKeys(line)
    else {
        return errorLine(nil, AgentError("invalid_json", "not a JSON object with unique keys"))
    }
    guard let id = object["id"] as? String else {
        return errorLine(nil, AgentError("missing_field", "`id` must be a string"))
    }
    guard let version = isPlainInteger(object["version"]) else {
        return errorLine(id, AgentError("missing_field", "`version` must be a non-negative integer"))
    }
    guard let method = object["method"] as? String else {
        return errorLine(id, AgentError("missing_field", "`method` must be a string"))
    }
    if version != protocolVersion {
        return errorLine(
            id, AgentError("version_mismatch", "this service speaks protocol version \(protocolVersion)"))
    }
    switch method {
    case "describe":
        var result: [String: Any] = [
            "app": "ReachAgent", "app_version": appVersion,
            "protocol_version": protocolVersion, "methods": methods, "test_mode": testMode,
        ]
        if let input = input {
            result["methods"] = methods + inputMethods
            result["input"] = input.describe()
        }
        return okLine(id, result)
    case "permissions":
        return okLine(id, provider.current())
    case "request_permissions":
        do {
            let names = try parseRequested(object["params"] ?? [String: Any](), inputEnabled: input != nil)
            let asked = provider.performsRequests ? names : []
            return okLine(id, ["requested": asked, "permissions": provider.request(asked)])
        } catch let error as AgentError {
            return errorLine(id, error)
        } catch {
            return errorLine(id, AgentError("invalid_params", "\(error)"))
        }
    case "capture", "capture_all_windows":
        return handleCapture(
            id, object, includeOffscreen: method == "capture_all_windows", provider: provider,
            backend: backend, input: input)
    case "accessibility", "accessibility_subtree":
        return handleAccessibility(
            id, object, subtree: method == "accessibility_subtree", provider: provider,
            backend: axBackend)
    case "action" where input != nil:
        return handleAction(id, object, provider: provider, input: input!)
    default:
        return errorLine(id, AgentError("unknown_method", "unknown method `\(method)`"))
    }
}

// MARK: - Socket service

func peerAllowed(peerUid: uid_t, ownUid: uid_t) -> Bool { peerUid == ownUid }

func makeAddress(_ path: String) -> sockaddr_un {
    var address = sockaddr_un()
    address.sun_family = sa_family_t(AF_UNIX)
    address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
    let bytes = Array(path.utf8CString)
    withUnsafeMutableBytes(of: &address.sun_path) { destination in
        for (index, byte) in bytes.enumerated() where index < destination.count {
            destination[index] = UInt8(bitPattern: byte)
        }
    }
    return address
}

func bindAddress(_ fd: Int32, _ path: String, connect: Bool) -> Int32 {
    var address = makeAddress(path)
    let length = socklen_t(MemoryLayout<sockaddr_un>.size)
    return withUnsafePointer(to: &address) { pointer in
        pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { base in
            connect ? Foundation.connect(fd, base, length) : Foundation.bind(fd, base, length)
        }
    }
}

/// Create the listening socket. Refuses to replace anything that is not our own socket.
func makeListener(path: String) throws -> Int32 {
    guard path.hasPrefix("/"), path.utf8.count <= maxSocketPathBytes else {
        throw AgentError("invalid_path", "the socket path must be absolute and at most \(maxSocketPathBytes) bytes")
    }
    let parent = (path as NSString).deletingLastPathComponent
    var parentInfo = stat()
    guard stat(parent, &parentInfo) == 0, (parentInfo.st_mode & S_IFMT) == S_IFDIR,
        parentInfo.st_uid == getuid(), (parentInfo.st_mode & 0o022) == 0
    else {
        throw AgentError("invalid_path", "the socket directory must be ours and not writable by others")
    }
    var info = stat()
    if lstat(path, &info) == 0 {
        guard (info.st_mode & S_IFMT) == S_IFSOCK, info.st_uid == getuid() else {
            throw AgentError("path_not_socket", "the path exists and is not our socket; it was left alone")
        }
        unlink(path)
    }
    let fd = socket(AF_UNIX, SOCK_STREAM, 0)
    guard fd >= 0 else { throw AgentError("socket_failed", "socket() failed") }
    let oldMask = umask(0o077)
    let bound = bindAddress(fd, path, connect: false)
    umask(oldMask)
    guard bound == 0, chmod(path, 0o600) == 0, listen(fd, 8) == 0 else {
        close(fd)
        if bound == 0 { unlink(path) }
        throw AgentError("socket_failed", "bind, chmod or listen failed")
    }
    return fd
}

func writeAll(_ fd: Int32, _ data: Data) -> Bool {
    var offset = 0
    let bytes = [UInt8](data)
    while offset < bytes.count {
        let written = bytes.withUnsafeBytes { raw in
            write(fd, raw.baseAddress! + offset, bytes.count - offset)
        }
        if written <= 0 { return false }
        offset += written
    }
    return true
}

func handleConnection(
    _ fd: Int32, provider: PermissionProvider, backend: CaptureBackend,
    axBackend: AccessibilityBackend, testMode: Bool, input: InputController? = nil
) {
    var timeout = timeval(tv_sec: 30, tv_usec: 0)
    setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size))
    setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size))
    var peerUid: uid_t = 0
    var peerGid: gid_t = 0
    guard getpeereid(fd, &peerUid, &peerGid) == 0, peerAllowed(peerUid: peerUid, ownUid: getuid()) else {
        let error = AgentError("peer_uid_mismatch", "the peer is not the user that runs the service")
        _ = writeAll(fd, Data((errorLine(nil, error) + "\n").utf8))
        return
    }
    var buffer = Data()
    var chunk = [UInt8](repeating: 0, count: 8192)
    while true {
        let count = read(fd, &chunk, chunk.count)
        if count <= 0 { return }
        buffer.append(contentsOf: chunk[0..<count])
        while let newline = buffer.firstIndex(of: 10) {
            let lineData = buffer[buffer.startIndex..<newline]
            if lineData.count > maxLineBytes {
                rejectTooLongLine(fd)
                return
            }
            buffer.removeSubrange(buffer.startIndex...newline)
            let reply: String
            if let line = String(data: lineData, encoding: .utf8) {
                reply = handleLine(
                    line, provider: provider, backend: backend, axBackend: axBackend,
                    testMode: testMode, input: input)
            } else {
                reply = errorLine(nil, AgentError("invalid_json", "the request is not UTF-8"))
            }
            if !writeAll(fd, Data((reply + "\n").utf8)) { return }
        }
        if buffer.count > maxLineBytes {
            rejectTooLongLine(fd)
            return
        }
    }
}

/// Answer an over-long request line with `invalid_json`; the caller then closes the connection.
/// The peer's remaining bytes are read (a bounded amount) first, so that closing with unread
/// data does not reset the connection before the peer has read the reply.
func rejectTooLongLine(_ fd: Int32) {
    let error = AgentError("invalid_json", "request line is too long")
    guard writeAll(fd, Data((errorLine(nil, error) + "\n").utf8)) else { return }
    shutdown(fd, SHUT_WR)
    var chunk = [UInt8](repeating: 0, count: 8192)
    for _ in 0..<16 {
        if read(fd, &chunk, chunk.count) <= 0 { break }
    }
}

/// Serve connections one after another. `maxConnections` stops the loop (self-test only).
/// Returns false when `accept` failed for good, true when `maxConnections` was reached.
@discardableResult
func serve(
    listenFd: Int32, provider: PermissionProvider, backend: CaptureBackend,
    axBackend: AccessibilityBackend, testMode: Bool, input: InputController? = nil, maxConnections: Int?
) -> Bool {
    var handled = 0
    while maxConnections == nil || handled < maxConnections! {
        let fd = accept(listenFd, nil, nil)
        if fd < 0 {
            // Retry only the transient errors; anything else (a closed listener, no descriptors)
            // would spin forever.
            if errno == EINTR || errno == ECONNABORTED { continue }
            fputs("accept failed (errno \(errno)); the service stops\n", stderr)
            return false
        }
        handleConnection(
            fd, provider: provider, backend: backend, axBackend: axBackend, testMode: testMode,
            input: input)
        close(fd)
        handled += 1
    }
    return true
}

// MARK: - Self-test

/// Send `lines`, read `expecting` replies; `peerClosed` (when given) tells whether the service
/// then closed the connection (end of stream) instead of waiting for more requests.
func clientExchange(
    path: String, lines: [Data], expecting: Int, peerClosed: ((Bool) -> Void)? = nil
) -> [[String: Any]]? {
    let fd = socket(AF_UNIX, SOCK_STREAM, 0)
    guard fd >= 0 else { return nil }
    defer { close(fd) }
    var timeout = timeval(tv_sec: 20, tv_usec: 0)
    setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size))
    setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size))
    guard bindAddress(fd, path, connect: true) == 0 else { return nil }
    for line in lines {
        var framed = line
        framed.append(10)
        guard writeAll(fd, framed) else { return nil }
    }
    var buffer = Data()
    var replies: [[String: Any]] = []
    var chunk = [UInt8](repeating: 0, count: 8192)
    while replies.count < expecting {
        let count = read(fd, &chunk, chunk.count)
        if count <= 0 { return nil }
        buffer.append(contentsOf: chunk[0..<count])
        while let newline = buffer.firstIndex(of: 10) {
            let lineData = buffer[buffer.startIndex..<newline]
            buffer.removeSubrange(buffer.startIndex...newline)
            guard let object = try? JSONSerialization.jsonObject(with: Data(lineData)),
                let reply = object as? [String: Any]
            else { return nil }
            replies.append(reply)
        }
    }
    if let peerClosed = peerClosed {
        peerClosed(read(fd, &chunk, chunk.count) == 0)
    }
    return replies
}

func selfTest() -> Never {
    var failures: [String] = []
    func check(_ condition: Bool, _ what: String) { if !condition { failures.append(what) } }
    func code(_ reply: [String: Any]) -> String? { (reply["error"] as? [String: Any])?["code"] as? String }
    func line(_ text: String) -> Data { Data(text.utf8) }

    var template = Array((NSTemporaryDirectory() + "reach-agent-XXXXXX").utf8CString)
    if template.count > maxSocketPathBytes - 10 { template = Array("/tmp/reach-agent-XXXXXX".utf8CString) }
    guard mkdtemp(&template) != nil else {
        fputs("self-test failed: cannot create a temporary directory\n", stderr)
        exit(1)
    }
    let directory = String(cString: template)
    let socketPath = directory + "/s"

    var info = stat()
    check(stat(directory, &info) == 0 && (info.st_mode & 0o777) == 0o700, "temporary directory mode is 0700")

    check(peerAllowed(peerUid: 501, ownUid: 501), "same uid is allowed")
    check(!peerAllowed(peerUid: 0, ownUid: 501), "other uid is refused")

    // An existing regular file must be refused and left alone.
    let plain = directory + "/plain.txt"
    FileManager.default.createFile(atPath: plain, contents: Data("keep".utf8))
    do {
        _ = try makeListener(path: plain)
        failures.append("makeListener accepted a regular file")
    } catch let error as AgentError {
        check(error.code == "path_not_socket", "regular file gives path_not_socket")
    } catch {
        failures.append("unexpected error for a regular file")
    }
    check(FileManager.default.fileExists(atPath: plain), "regular file still exists")
    unlink(plain)

    signal(SIGPIPE, SIG_IGN)
    guard let listener = try? makeListener(path: socketPath) else {
        fputs("self-test failed: cannot listen on the temporary socket\n", stderr)
        rmdir(directory)
        exit(1)
    }
    check(stat(socketPath, &info) == 0 && (info.st_mode & 0o777) == 0o600, "socket mode is 0600")

    let provider = SelfTestPermissionProvider()
    let done = DispatchSemaphore(value: 0)
    Thread.detachNewThread {
        serve(
            listenFd: listener, provider: provider, backend: FakeCaptureBackend(),
            axBackend: FakeAccessibilityBackend(), testMode: true, maxConnections: 6)
        done.signal()
    }

    let first = clientExchange(
        path: socketPath,
        lines: [
            line(#"{"version":1,"id":"1","method":"describe"}"#),
            line(#"{"version":1,"id":"2","method":"permissions"}"#),
            line(#"{"version":1,"id":"3","method":"request_permissions","params":{"permissions":["accessibility"]}}"#),
            line(#"{"version":1,"id":"4","method":"request_permissions","params":{"permissions":["post_event"]}}"#),
            line(#"{"version":1,"id":"5","method":"request_permissions","params":{"permissions":[]}}"#),
        ], expecting: 5)
    if let r = first {
        let describe = r[0]["result"] as? [String: Any]
        check(r[0]["ok"] as? Bool == true && describe?["protocol_version"] as? Int == protocolVersion, "describe")
        check(describe?["test_mode"] as? Bool == true, "describe reports test mode")
        check(describe?["methods"] as? [String] == methods, "describe lists the methods")
        let perms = r[1]["result"] as? [String: String]
        check(perms == ["accessibility": "unknown", "screen_recording": "unknown", "post_event": "unknown"],
            "permissions are unknown in test mode")
        let requested = r[2]["result"] as? [String: Any]
        check((requested?["requested"] as? [String])?.isEmpty == true, "test mode requests nothing")
        check(requested?["permissions"] as? [String: String] == perms, "request keeps states unknown")
        check(code(r[3]) == "not_requestable", "post_event is not requestable")
        check(code(r[4]) == "invalid_params", "empty permission list is invalid")
    } else {
        failures.append("connection 1 failed")
    }

    let second = clientExchange(
        path: socketPath,
        lines: [
            line(#"{"version":2,"id":"a","method":"describe"}"#),
            line("not json"),
            line(#"{"version":1,"id":"c","method":"nope"}"#),
            line(#"{"version":1,"method":"describe"}"#),
            line(#"{"version":1,"version":1,"id":"e","method":"describe"}"#),
        ], expecting: 5)
    if let r = second {
        check(code(r[0]) == "version_mismatch", "version mismatch")
        check(code(r[1]) == "invalid_json", "not json")
        check(code(r[2]) == "unknown_method", "unknown method")
        check(code(r[3]) == "missing_field", "missing id")
        check(code(r[4]) == "invalid_json", "duplicate key")
    } else {
        failures.append("connection 2 failed")
    }

    var thirdClosed = false
    let third = clientExchange(
        path: socketPath, lines: [Data(repeating: 0x78, count: maxLineBytes + 1)], expecting: 1,
        peerClosed: { thirdClosed = $0 })
    check(third.map { code($0[0]) } == "invalid_json", "over-long line")
    check(thirdClosed, "over-long line closes the connection")

    // Connection 4: `screen_recording` is unknown, so no capture may run and no file may appear.
    let deniedPath = directory + "/denied.png"
    let fourth = clientExchange(
        path: socketPath,
        lines: [
            line(#"{"version":1,"id":"p","method":"capture","params":{"pid":42,"window_id":7,"image":"\#(deniedPath)"}}"#),
            line(#"{"version":1,"id":"q","method":"capture","params":{"pid":42,"window_id":7,"image":"\#(deniedPath)","extra":1}}"#),
            line(#"{"version":1,"id":"r","method":"capture_all_windows","params":{"pid":0,"window_id":7,"image":"\#(deniedPath)"}}"#),
            line(#"{"version":1,"id":"s","method":"capture"}"#),
            line(#"{"version":1,"id":"t","method":"accessibility","params":{"pid":42}}"#),
            line(#"{"version":1,"id":"u","method":"accessibility_subtree","params":{"pid":42,"source":"main","path":[],"extra":1}}"#),
            line(#"{"version":1,"id":"v","method":"accessibility","params":{"pid":0}}"#),
            line(#"{"version":1,"id":"w","method":"accessibility_subtree","params":{"pid":42,"source":"windows","path":[]}}"#),
            line(#"{"version":1,"id":"x","method":"accessibility","params":{"pid":42,"windows_only":1}}"#),
            line(#"{"version":1,"id":"y","method":"accessibility_subtree","params":{"pid":42,"source":"main","path":[0]}}"#),
            line(#"{"version":1,"id":"z","method":"accessibility","params":{"pid":42,"ax_helper":"/x"}}"#),
        ], expecting: 11)
    if let r = fourth {
        let axDenied = r[4]["error"] as? [String: Any]
        check(code(r[4]) == "permission_denied", "accessibility without permission is denied")
        check(axDenied?["permission"] as? String == "accessibility", "denial names accessibility")
        check(axDenied?["state"] as? String == "unknown", "accessibility denial reports the state")
        check(code(r[5]) == "invalid_params", "extra subtree parameter is invalid (before the permission)")
        check(code(r[6]) == "invalid_params", "accessibility pid 0 is invalid")
        check(code(r[7]) == "invalid_params", "a windows subtree needs a window index")
        check(code(r[8]) == "invalid_params", "windows_only must be a boolean")
        let axSubtreeDenied = r[9]["error"] as? [String: Any]
        check(code(r[9]) == "permission_denied", "accessibility_subtree without permission is denied")
        check(axSubtreeDenied?["permission"] as? String == "accessibility" && axSubtreeDenied?["state"] as? String == "unknown",
            "subtree denial names accessibility and its state")
        check(code(r[10]) == "invalid_params", "an extra accessibility parameter is invalid (before the permission)")
        let denied = r[0]["error"] as? [String: Any]
        check(code(r[0]) == "permission_denied", "capture without permission is denied")
        check(denied?["permission"] as? String == "screen_recording", "denial names screen_recording")
        check(denied?["state"] as? String == "unknown", "denial reports the state")
        check(code(r[1]) == "invalid_params", "extra capture parameter is invalid (before the permission)")
        check(code(r[2]) == "invalid_params", "pid 0 is invalid")
        check(code(r[3]) == "invalid_params", "missing capture params are invalid")
    } else {
        failures.append("connection 4 failed")
    }
    check(!FileManager.default.fileExists(atPath: deniedPath), "no file is created when the permission is missing")

    // Connection 5: with the permissions granted, the fake backends answer.
    provider.screenRecording = "granted"
    provider.accessibility = "granted"
    let shotPath = directory + "/shot.png"
    let allPath = directory + "/all.png"
    let keptPath = directory + "/kept.png"
    let missingPath = directory + "/missing.png"
    FileManager.default.createFile(atPath: keptPath, contents: Data("keep".utf8))
    let fifth = clientExchange(
        path: socketPath,
        lines: [
            line(#"{"version":1,"id":"a","method":"capture","params":{"pid":42,"window_id":7,"image":"\#(shotPath)"}}"#),
            line(#"{"version":1,"id":"b","method":"capture","params":{"pid":42,"window_id":7,"image":"\#(shotPath)"}}"#),
            line(#"{"version":1,"id":"c","method":"capture_all_windows","params":{"pid":42,"window_id":7,"image":"\#(allPath)"}}"#),
            line(#"{"version":1,"id":"d","method":"capture","params":{"pid":42,"window_id":4242,"image":"\#(missingPath)"}}"#),
            line(#"{"version":1,"id":"e","method":"capture","params":{"pid":42,"window_id":7,"image":"\#(keptPath)"}}"#),
            line(#"{"version":1,"id":"f","method":"describe"}"#),
            line(#"{"version":1,"id":"g","method":"accessibility","params":{"pid":42}}"#),
            line(#"{"version":1,"id":"h","method":"accessibility","params":{"pid":42,"windows_only":true}}"#),
            line(#"{"version":1,"id":"i","method":"accessibility","params":{"pid":4242}}"#),
            line(#"{"version":1,"id":"j","method":"accessibility_subtree","params":{"pid":42,"source":"main","path":[0,1]}}"#),
            line(#"{"version":1,"id":"k","method":"accessibility_subtree","params":{"pid":42,"source":"focused","path":[99]}}"#),
        ], expecting: 11)
    if let r = fifth {
        let axFull = r[6]["result"] as? [String: Any]
        check(r[6]["ok"] as? Bool == true && axFull?["pid"] as? Int == 42, "accessibility succeeds with the permission")
        check(axFull?["tree_scope"] as? String == "full_bounded_tree", "accessibility reports the full tree scope")
        check((axFull?["windows"] as? [Any])?.count == 1, "accessibility reports one window")
        check(axFull?["action_allowed"] as? Bool == false, "accessibility never allows actions")
        check(axFull?["cg_window_binding"] as? String == "unverified", "accessibility reports an unverified window binding")
        let axWindows = r[7]["result"] as? [String: Any]
        check(axWindows?["tree_scope"] as? String == "windows_only", "windows_only is honored")
        check(code(r[8]) == "ax_failed", "a backend failure is ax_failed")
        check((r[8]["error"] as? [String: Any])?["message"] as? String == "process_unavailable: process is not a running application or its start time is unavailable",
            "ax_failed carries the status and the reason")
        let axTree = r[9]["result"] as? [String: Any]
        check(r[9]["ok"] as? Bool == true && axTree?["path"] as? [Int] == [0, 1] && axTree?["source"] as? String == "main",
            "accessibility_subtree echoes the request")
        check(axTree?["scope"] as? String == "read_only_subtree_unverified", "subtree scope is unverified")
        check(code(r[10]) == "ax_failed", "an unavailable subtree is ax_failed")
        let result = r[0]["result"] as? [String: Any]
        check(r[0]["ok"] as? Bool == true, "capture succeeds with the permission")
        check(result?["window_id"] as? Int == 7 && result?["owner_pid"] as? Int == 42, "capture echoes the window")
        check(result?["image_width"] as? Int == 2 && result?["image_height"] as? Int == 2, "capture reports the size")
        check(result?["window_scope"] as? String == "onscreen_candidates", "capture uses the onscreen scope")
        check(result?["occlusion_checked"] as? Bool == false, "occlusion is not checked")
        check(result?["image_path"] as? String == shotPath, "capture reports the image path")
        check(code(r[1]) == "capture_failed", "a second capture to the same path fails")
        let all = r[2]["result"] as? [String: Any]
        check(all?["window_scope"] as? String == "all_window_candidates", "capture_all_windows uses the all-window scope")
        check(code(r[3]) == "capture_failed", "a missing window fails")
        check(code(r[4]) == "capture_failed", "an existing file is not overwritten")
        let describe = r[5]["result"] as? [String: Any]
        check(describe?["methods"] as? [String] == methods, "describe lists the capture methods")
    } else {
        failures.append("connection 5 failed")
    }
    let shot = FileManager.default.contents(atPath: shotPath)
    check(shot.map { Array($0.prefix(4)) } == [0x89, 0x50, 0x4E, 0x47], "the image is a PNG")
    check(stat(shotPath, &info) == 0 && (info.st_mode & 0o777) == 0o600, "the image mode is 0600")
    check(FileManager.default.contents(atPath: keptPath) == Data("keep".utf8), "the existing file is unchanged")
    check(!FileManager.default.fileExists(atPath: missingPath), "no file is created for a missing window")

    // Connection 6: each method checks its own permission. `accessibility` is denied while
    // `screen_recording` stays granted.
    provider.accessibility = "denied"
    let sixthPath = directory + "/sixth.png"
    let sixth = clientExchange(
        path: socketPath,
        lines: [
            line(#"{"version":1,"id":"a","method":"accessibility","params":{"pid":42}}"#),
            line(#"{"version":1,"id":"b","method":"accessibility_subtree","params":{"pid":42,"source":"main","path":[]}}"#),
            line(#"{"version":1,"id":"c","method":"capture","params":{"pid":42,"window_id":7,"image":"\#(sixthPath)"}}"#),
        ], expecting: 3)
    if let r = sixth {
        let denied = r[0]["error"] as? [String: Any]
        check(code(r[0]) == "permission_denied", "accessibility is denied when its permission is denied")
        check(denied?["permission"] as? String == "accessibility" && denied?["state"] as? String == "denied",
            "the denial names accessibility and its state")
        check(code(r[1]) == "permission_denied", "the subtree is denied too")
        check(r[2]["ok"] as? Bool == true, "capture still works with screen_recording granted")
    } else {
        failures.append("connection 6 failed")
    }

    for failure in accessibilitySelfCheck() { failures.append("accessibility self-check: \(failure)") }
    for failure in inputSelfCheck() { failures.append("input self-check: \(failure)") }

    // The system input backend is unreachable in test mode, under CI and without input.
    check(SystemInputLaunch.authorize(enableInput: true, testMode: true, environment: [:]) == nil,
        "test mode never authorizes the system input backend")
    check(SystemInputLaunch.authorize(enableInput: true, testMode: false, environment: ["CI": "true"]) == nil,
        "CI never authorizes the system input backend")
    check(SystemInputLaunch.authorize(enableInput: false, testMode: false, environment: [:]) == nil,
        "no --enable-input never authorizes the system input backend")
    let ciConfig = try? parseServeArguments([
        "--serve", "--socket", "/tmp/x", "--enable-input", "--allow-bundle", fixtureBundleId,
        "--input-ttl-seconds", "60", "--max-input-events", "1",
    ])
    if let ciConfig = ciConfig {
        do {
            _ = try makeInputController(ciConfig, environment: ["CI": "1"])
            failures.append("--enable-input without --test-mode started under CI")
        } catch let error as AgentError {
            check(error.code == "input_unavailable", "--enable-input under CI fails at start-up")
        } catch {
            failures.append("unexpected start-up error under CI")
        }
        var testConfig = ciConfig
        testConfig = ServeConfig(socket: ciConfig.socket, testMode: true, input: ciConfig.input)
        let made = try? makeInputController(testConfig, environment: ["CI": "1"])
        check(made?.controller?.backendName == "fake" && made?.launch == nil, "--test-mode --enable-input uses the fake backend")
        check(
            (try? makeInputController(ServeConfig(socket: "/tmp/x", testMode: false, input: nil), environment: [:]))
                .map { $0.controller == nil && $0.launch == nil } == true,
            "without --enable-input there is no input controller")
    } else {
        failures.append("the CI configuration did not parse")
    }
    check((try? parseRequested(["permissions": ["post_event"]], inputEnabled: true)) == ["post_event"],
        "post_event is requestable only with input enabled")

    // Input-enabled service on its own socket: fake input backend, fake capture, fake clock.
    let inputSocket = directory + "/i"
    var clock: UInt64 = 1_000
    let clockLock = NSLock()
    let fakeNow: () -> UInt64 = {
        clockLock.lock()
        defer { clockLock.unlock() }
        return clock
    }
    let inputArming = InputArming(
        InputOptions(bundles: [fixtureBundleId], keycodes: [18], ttlSeconds: 60, maxEvents: 2),
        nowNs: 1_000, nowUnixSeconds: 1_700_000_000)!
    let fakeInput = makeFakeInputController(arming: inputArming, now: fakeNow)
    let inputProvider = SelfTestPermissionProvider()
    inputProvider.screenRecording = "granted"
    let inputDone = DispatchSemaphore(value: 0)
    let evidence = directory + "/evidence"
    let evidence2 = directory + "/evidence2"
    let inputShot = directory + "/input.png"
    let inputShot2 = directory + "/input2.png"
    let inputAll = directory + "/input-all.png"
    if let inputListener = try? makeListener(path: inputSocket) {
        Thread.detachNewThread {
            serve(
                listenFd: inputListener, provider: inputProvider, backend: FakeCaptureBackend(),
                axBackend: FakeAccessibilityBackend(), testMode: true, input: fakeInput.controller,
                maxConnections: 3)
            inputDone.signal()
        }
        let pre = clientExchange(
            path: inputSocket,
            lines: [
                line(#"{"version":1,"id":"a","method":"describe"}"#),
                line(#"{"version":1,"id":"b","method":"action","params":{}}"#),
            ], expecting: 2)
        if let r = pre {
            let d = r[0]["result"] as? [String: Any]
            let block = d?["input"] as? [String: Any]
            check(d?["methods"] as? [String] == methods + ["action"], "describe lists action with input enabled")
            check(block?["state"] as? String == "armed" && block?["backend"] as? String == "fake", "describe reports armed input")
            check(block?["allowed_bundle_ids"] as? [String] == [fixtureBundleId] && block?["allowed_keycodes"] as? [Int] == [18],
                "describe lists the allowed bundle ids and keycodes")
            check(block?["events_remaining"] as? Int == 2 && block?["expires_unix_seconds"] as? Double == 1_700_000_060,
                "describe reports the remaining events and the expiry")
            let schema = r[1]["result"] as? [String: Any]
            check(schema?["status"] as? String == "refused" && schema?["gate"] as? String == "schema", "empty action params are refused")
        } else {
            failures.append("input connection 1 failed")
        }

        // post_event unknown: a valid action is denied before the gate.
        let sha2x2 = "0000000000000000000000000000000000000000000000000000000000000000"
        func action(_ id: String, _ sha: String, _ dir: String, _ event: String = #"{"kind":"click","button":"left","x":1,"y":1}"#) -> Data {
            line(#"{"version":1,"id":"\#(id)","method":"action","params":{"delivery":"foreground","target":{"bundle_id":"\#(fixtureBundleId)","pid":42,"window_id":7,"process_start_unix_seconds":1000.5},"observation":{"image_sha256":"\#(sha)"},"event":\#(event),"evidence_dir":"\#(dir)"}}"#)
        }
        let denied = clientExchange(path: inputSocket, lines: [action("c", sha2x2, evidence)], expecting: 1)
        check(denied.map { code($0[0]) } == "permission_denied", "action needs post_event")
        check(!FileManager.default.fileExists(atPath: evidence), "a denied action writes nothing")

        inputProvider.postEvent = "granted"
        let capture = clientExchange(
            path: inputSocket,
            lines: [
                line(#"{"version":1,"id":"d","method":"capture","params":{"pid":42,"window_id":7,"image":"\#(inputShot)"}}"#)
            ], expecting: 1)
        let sha = (capture?[0]["result"] as? [String: Any])?["image_sha256"] as? String ?? ""
        check(sha.count == 64 && sha == fileSha256(inputShot), "capture reports the hash of its own file with input enabled")
        // Connection 3 was the last one; the remaining checks call handleLine directly.
        func call(_ data: Data) -> [String: Any] {
            let text = String(data: data, encoding: .utf8)!
            let reply = handleLine(
                text, provider: inputProvider, backend: FakeCaptureBackend(), axBackend: FakeAccessibilityBackend(),
                testMode: true, input: fakeInput.controller)
            return (try? JSONSerialization.jsonObject(with: Data(reply.utf8))) as? [String: Any] ?? [:]
        }
        func outcome(_ reply: [String: Any]) -> String {
            let r = reply["result"] as? [String: Any]
            return "\(r?["status"] as? String ?? "?")/\(r?["gate"] as? String ?? "-")/\(r?["reason"] as? String ?? "-")"
        }
        check(outcome(call(action("e", sha2x2, evidence))) == "refused/G5/observation_mismatch",
            "an observation that is not the latest capture is refused")
        let posted = call(action("f", sha, evidence))
        check(outcome(posted) == "posted_unverified/-/-", "a click on the captured fixture window is posted to the fake backend")
        check((posted["result"] as? [String: Any])?["action_allowed"] as? Bool == false, "the action reply never allows actions")
        check(fakeInput.fake.events == ["down 1.0 1.0", "up 1.0 1.0"], "the fake backend recorded one press and one release")
        for name in ["attempt.json", "result.json"] {
            check(stat(evidence + "/" + name, &info) == 0 && (info.st_mode & 0o777) == 0o600, "\(name) is written with mode 0600")
        }
        check(stat(evidence, &info) == 0 && (info.st_mode & 0o777) == 0o700, "the evidence directory has mode 0700")
        check(outcome(call(action("f", sha, evidence))) == "refused/G13/request_already_executed", "a replay through the socket is refused")
        check(outcome(call(action("g", sha, evidence2))) == "refused/G5/observation_already_used", "a spent observation is refused")
        // A new capture writes the same synthetic image, so its hash is spent too.
        _ = call(line(#"{"version":1,"id":"h","method":"capture_all_windows","params":{"pid":42,"window_id":7,"image":"\#(inputAll)"}}"#))
        check(outcome(call(action("i", sha, evidence2))) == "refused/G5/observation_already_used",
            "capture_all_windows does not create an observation")
        _ = call(line(#"{"version":1,"id":"j","method":"capture","params":{"pid":42,"window_id":7,"image":"\#(inputShot2)"}}"#))
        check(outcome(call(action("k", sha, evidence2, #"{"kind":"key_tap","keycode":49}"#))) == "refused/G5/observation_already_used",
            "the identical synthetic capture is still spent")
        clockLock.lock()
        clock = 1_000 + 61_000_000_000
        clockLock.unlock()
        let late = call(line(#"{"version":1,"id":"l","method":"describe"}"#))
        let lateBlock = (late["result"] as? [String: Any])?["input"] as? [String: Any]
        check(lateBlock?["state"] as? String == "disabled" && lateBlock?["events_remaining"] as? Int == 1,
            "describe reports the expired arming as disabled")
        check(outcome(call(action("m", sha, evidence2))) == "refused/G1/arming_expired", "an expired arming refuses through the service")
        check(fakeInput.fake.events.count == 2, "only the one accepted click reached the fake backend")
        check(!FileManager.default.fileExists(atPath: evidence2), "refused actions write no receipt")
        check(inputDone.wait(timeout: .now() + 10) == .success, "the input service stopped after three connections")
        close(inputListener)
    } else {
        failures.append("cannot listen on the input socket")
    }
    for path in [evidence + "/attempt.json", evidence + "/result.json", inputShot, inputShot2, inputAll, inputSocket] {
        unlink(path)
    }
    rmdir(evidence)

    // Without --enable-input `action` stays an unknown method and post_event is not requestable.
    let plainAction = handleLine(
        #"{"version":1,"id":"n","method":"action","params":{}}"#, provider: provider, backend: FakeCaptureBackend(),
        axBackend: FakeAccessibilityBackend(), testMode: true)
    check(plainAction.contains("\"unknown_method\""), "action is an unknown method without --enable-input")

    check(done.wait(timeout: .now() + 10) == .success, "service stopped after six connections")
    close(listener)
    for path in [shotPath, allPath, keptPath, sixthPath] { unlink(path) }
    unlink(socketPath)
    // `exit` does not run `defer`, so the directory is removed explicitly.
    check(rmdir(directory) == 0, "temporary directory removed")

    if failures.isEmpty {
        print("ReachAgent self-test passed; no screen capture, no accessibility read, no permission prompt, no input event (fake input backend only)")
        exit(0)
    }
    for failure in failures { fputs("self-test failed: \(failure)\n", stderr) }
    exit(1)
}

// MARK: - Entry point

func usage(_ problem: String? = nil) -> Never {
    if let problem = problem { fputs("ReachAgent: \(problem)\n", stderr) }
    fputs(
        """
        usage: ReachAgent --self-test
               ReachAgent --serve --socket <absolute path> [--test-mode]
                   [--enable-input --allow-bundle <bundle id>... [--allow-keycode <0-127>...]
                    --input-ttl-seconds <1-600> --max-input-events <1-20>]
        Input is disabled unless --enable-input is given. Allowed bundle ids: \(approvedInputBundleIds.joined(separator: ", ")).

        """, stderr)
    exit(2)
}

/// The input controller for `--serve`, or nil when input is not enabled. The system backend
/// is used only when `SystemInputLaunch.authorize` allows it; `--test-mode` gets the fake
/// backend, and under CI start-up fails.
func makeInputController(_ config: ServeConfig, environment: [String: String]) throws -> (
    controller: InputController?, launch: SystemInputLaunch?
) {
    guard let options = config.input else { return (nil, nil) }
    guard let arming = InputArming(options, nowNs: monotonicNs(), nowUnixSeconds: Date().timeIntervalSince1970)
    else {
        throw AgentError("invalid_arming", "the input options are outside the approved limits")
    }
    if config.testMode {
        return (makeFakeInputController(arming: arming, now: monotonicNs).controller, nil)
    }
    guard let launch = SystemInputLaunch.authorize(enableInput: true, testMode: false, environment: environment)
    else {
        throw AgentError("input_unavailable", "--enable-input without --test-mode is refused under CI")
    }
    let system = makeSystemInputBackend(launch)
    let controller = InputController(
        arming: arming, backendName: "system", sampler: system.sampler, poster: system.poster,
        sink: FileReceiptSink(), now: monotonicNs)
    return (controller, launch)
}

@main
struct ReachAgent {
    static func main() {
        let args = Array(CommandLine.arguments.dropFirst())
        if args == ["--self-test"] { selfTest() }
        let config: ServeConfig
        do {
            config = try parseServeArguments(args)
        } catch {
            usage("\(error)")
        }
        let testMode = config.testMode
        signal(SIGPIPE, SIG_IGN)
        do {
            let input = try makeInputController(config, environment: ProcessInfo.processInfo.environment)
            let listener = try makeListener(path: config.socket)
            let provider: PermissionProvider =
                testMode ? TestPermissionProvider() : SystemPermissionProvider(inputLaunch: input.launch)
            let backend: CaptureBackend = testMode ? FakeCaptureBackend() : SystemCaptureBackend()
            let axBackend: AccessibilityBackend =
                testMode ? FakeAccessibilityBackend() : SystemAccessibilityBackend()
            if !testMode { prepareScreenCapture() }
            let stoppedNormally = serve(
                listenFd: listener, provider: provider, backend: backend, axBackend: axBackend,
                testMode: testMode, input: input.controller, maxConnections: nil)
            if !stoppedNormally {
                close(listener)
                unlink(config.socket)
                exit(1)
            }
        } catch let error as AgentError {
            fputs("ReachAgent: \(error.code): \(error.message)\n", stderr)
            exit(2)
        } catch {
            fputs("ReachAgent: \(error)\n", stderr)
            exit(2)
        }
    }
}
