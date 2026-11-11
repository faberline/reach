// Window capture for the ReachAgent service (faberline/reach-old#35). It is compiled together with
// ReachAgent.swift. Read-only ScreenCaptureKit: no input events, no foreground change.
// It never asks for a permission: the service pre-flights `screen_recording` before it calls
// a backend, so a backend runs only when the permission is already granted.
import AppKit
import Foundation
import ScreenCaptureKit

struct CaptureFailure: Error, CustomStringConvertible {
    let description: String
    init(_ message: String) { self.description = message }
}

struct CaptureRequest {
    let windowId: UInt32
    let pid: Int32
    let output: String
    let includeOffscreen: Bool
}

/// Contract: create `request.output` exclusively (mode 0600, no overwrite, no symlink following)
/// and return the report. Any failure throws `CaptureFailure`; `writeNew` removes a partial file.
protocol CaptureBackend: Sendable {
    func capture(_ request: CaptureRequest) async throws -> [String: Any]
}

func rectValues(_ r: CGRect) -> [String: Double] {
    ["x": r.origin.x, "y": r.origin.y, "width": r.width, "height": r.height]
}

func pngData(_ image: CGImage) throws -> Data {
    guard let png = NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:]) else {
        throw CaptureFailure("cannot encode PNG")
    }
    return png
}

func writeNew(_ data: Data, path: String) throws {
    // No overwrite or symlink following. A partial new file is removed on error.
    let fd = open(path, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW, 0o600)
    guard fd >= 0 else { throw CaptureFailure("cannot create output: \(String(cString: strerror(errno)))") }
    var success = false
    defer { close(fd); if !success { unlink(path) } }
    try data.withUnsafeBytes { bytes in
        var offset = 0
        while offset < bytes.count {
            let n = Darwin.write(fd, bytes.baseAddress!.advanced(by: offset), bytes.count - offset)
            if n < 0 && errno == EINTR { continue }
            guard n > 0 else { throw CaptureFailure("failed writing image") }
            offset += n
        }
    }
    guard fsync(fd) == 0 else { throw CaptureFailure("failed syncing image") }
    success = true
}

// MARK: - System backend

@available(macOS 14.0, *)
func matchingWindow(id: CGWindowID, pid: pid_t, includeOffscreen: Bool) async throws -> SCWindow {
    let list = try await SCShareableContent.excludingDesktopWindows(true, onScreenWindowsOnly: !includeOffscreen)
    let matches = list.windows.filter { $0.windowID == id && $0.owningApplication?.processID == pid }
    guard matches.count == 1, let win = matches.first else {
        throw CaptureFailure("window missing or ambiguous for requested scope")
    }
    return win
}

@available(macOS 14.0, *)
func captureWindow(_ request: CaptureRequest) async throws -> [String: Any] {
    let id = request.windowId
    let pid = request.pid
    let includeOffscreen = request.includeOffscreen
    let before = try await matchingWindow(id: id, pid: pid, includeOffscreen: includeOffscreen)
    let frame = before.frame
    let onscreenBefore = before.isOnScreen
    guard frame.width.isFinite, frame.height.isFinite, frame.width > 0, frame.height > 0 else {
        throw CaptureFailure("invalid window dimensions")
    }
    let filter = SCContentFilter(desktopIndependentWindow: before)
    let scale = Double(filter.pointPixelScale)
    let rect = filter.contentRect
    guard scale.isFinite, scale > 0, rect.width.isFinite, rect.height.isFinite,
          rect.width > 0, rect.height > 0 else { throw CaptureFailure("invalid content geometry") }
    let width = (rect.width * scale).rounded(.up)
    let height = (rect.height * scale).rounded(.up)
    guard width >= 1, height >= 1, width <= 8192, height <= 8192 else {
        throw CaptureFailure("unsupported capture dimensions")
    }
    let config = SCStreamConfiguration()
    config.width = Int(width)
    config.height = Int(height)
    config.showsCursor = false
    let start = DispatchTime.now().uptimeNanoseconds
    let image = try await SCScreenshotManager.captureImage(contentFilter: filter, configuration: config)
    let end = DispatchTime.now().uptimeNanoseconds
    let wall = Date().timeIntervalSince1970
    let after = try await matchingWindow(id: id, pid: pid, includeOffscreen: includeOffscreen)
    guard after.frame == frame else { throw CaptureFailure("window geometry changed during capture") }
    guard after.isOnScreen == onscreenBefore else { throw CaptureFailure("window onscreen state changed during capture") }
    guard image.width == config.width, image.height == config.height else {
        throw CaptureFailure("capture dimensions differ from request")
    }
    let png = try pngData(image)
    let report: [String: Any] = [
        "window_id": id, "owner_pid": pid, "window_frame_points": rectValues(frame),
        "filter_content_rect_points": rectValues(rect), "point_pixel_scale": scale,
        "image_width": image.width, "image_height": image.height,
        "capture_start_uptime_ns": start, "capture_end_uptime_ns": end,
        "capture_end_unix_seconds": wall, "image_path": request.output,
        "capture_type": "desktop_independent_window",
        "window_scope": includeOffscreen ? "all_window_candidates" : "onscreen_candidates",
        "onscreen_before": onscreenBefore, "onscreen_after": after.isOnScreen,
        "occlusion_checked": false,
    ]
    // A request that already timed out must not create the file afterwards.
    try Task.checkCancellation()
    try writeNew(png, path: request.output)
    return report
}

/// The real backend. It is created only by `--serve` outside test mode.
struct SystemCaptureBackend: CaptureBackend {
    func capture(_ request: CaptureRequest) async throws -> [String: Any] {
        guard #available(macOS 14.0, *) else { throw CaptureFailure("macOS 14 or later required") }
        do {
            return try await captureWindow(request)
        } catch is CancellationError {
            throw CaptureFailure("capture was cancelled")
        }
    }
}

// MARK: - Fake backend

/// Used by --self-test and --test-mode: a synthetic 2x64 PNG, no screen access.
/// It is 64 points tall so that the fake window has points below the title-bar band (G6).
/// Window id 4242 stands for "window missing".
struct FakeCaptureBackend: CaptureBackend {
    func capture(_ request: CaptureRequest) async throws -> [String: Any] {
        if request.windowId == 4242 {
            throw CaptureFailure("window missing or ambiguous for requested scope")
        }
        guard let context = CGContext(data: nil, width: 2, height: 64, bitsPerComponent: 8,
                                      bytesPerRow: 8, space: CGColorSpaceCreateDeviceRGB(),
                                      bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue),
              let image = context.makeImage() else { throw CaptureFailure("cannot create synthetic image") }
        try writeNew(try pngData(image), path: request.output)
        let frame = rectValues(CGRect(x: 0, y: 0, width: 2, height: 64))
        return [
            "window_id": request.windowId, "owner_pid": request.pid, "window_frame_points": frame,
            "filter_content_rect_points": frame, "point_pixel_scale": 1.0,
            "image_width": 2, "image_height": 64,
            "capture_start_uptime_ns": 1, "capture_end_uptime_ns": 2,
            "capture_end_unix_seconds": 0.0, "image_path": request.output,
            "capture_type": "desktop_independent_window",
            "window_scope": request.includeOffscreen ? "all_window_candidates" : "onscreen_candidates",
            "onscreen_before": true, "onscreen_after": true, "occlusion_checked": false,
        ]
    }
}

// MARK: - Helpers for the service

final class CaptureResultBox: @unchecked Sendable {
    private let lock = NSLock()
    private var value: Result<[String: Any], CaptureFailure>?
    func set(_ newValue: Result<[String: Any], CaptureFailure>) {
        lock.lock()
        value = newValue
        lock.unlock()
    }
    func get() -> Result<[String: Any], CaptureFailure>? {
        lock.lock()
        defer { lock.unlock() }
        return value
    }
}

/// Run async work and wait for it on the calling thread. The service thread blocks in
/// `accept`, so the work must not need the main actor.
func runBlocking(
    timeoutSeconds: Double, _ work: @escaping @Sendable () async throws -> [String: Any]
) -> Result<[String: Any], CaptureFailure> {
    let box = CaptureResultBox()
    let done = DispatchSemaphore(value: 0)
    let task = Task.detached {
        do {
            box.set(.success(try await work()))
        } catch let failure as CaptureFailure {
            box.set(.failure(failure))
        } catch {
            box.set(.failure(CaptureFailure("\(error)")))
        }
        done.signal()
    }
    if done.wait(timeout: .now() + timeoutSeconds) == .timedOut {
        task.cancel()
        return .failure(CaptureFailure("capture timed out after \(Int(timeoutSeconds)) seconds"))
    }
    return box.get() ?? .failure(CaptureFailure("capture produced no result"))
}

/// ScreenCaptureKit needs AppKit's application context. The App runs as `LSUIElement`, so the
/// activation policy is left alone.
func prepareScreenCapture() {
    _ = NSApplication.shared
}
