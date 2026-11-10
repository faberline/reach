// Read-only ScreenCaptureKit helper; no input events or foreground changes.
import Foundation
import AppKit
import ScreenCaptureKit
import Darwin

struct CaptureFailure: Error, CustomStringConvertible {
    let description: String
    let code: Int32
    init(_ message: String, _ code: Int32 = 1) { self.description = message; self.code = code }
}

@MainActor
@available(macOS 14.0, *)
func matchingWindow(id: CGWindowID, pid: pid_t, includeOffscreen: Bool) async throws -> SCWindow {
    let list = try await SCShareableContent.excludingDesktopWindows(true, onScreenWindowsOnly: !includeOffscreen)
    let matches = list.windows.filter { $0.windowID == id && $0.owningApplication?.processID == pid }
    guard matches.count == 1, let win = matches.first else {
        throw CaptureFailure("window missing or ambiguous for requested scope")
    }
    return win
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

@MainActor
@available(macOS 14.0, *)
func capture(id: CGWindowID, pid: pid_t, output: String, includeOffscreen: Bool) async throws {
    // Check only: never request Screen Recording permission or show a prompt.
    guard CGPreflightScreenCaptureAccess() else { throw CaptureFailure("screen recording permission is not granted", 77) }
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
        "capture_end_unix_seconds": wall, "image_path": output,
        "capture_type": "desktop_independent_window", "window_scope": includeOffscreen ? "all_window_candidates" : "onscreen_candidates",
        "onscreen_before": onscreenBefore, "onscreen_after": after.isOnScreen, "occlusion_checked": false
    ]
    let encoded = try JSONSerialization.data(withJSONObject: report, options: [.sortedKeys])
    try writeNew(png, path: output)
    FileHandle.standardOutput.write(encoded)
    FileHandle.standardOutput.write(Data([10]))
}

@main
struct Main {
    @MainActor
    static func main() async {
        let args = CommandLine.arguments
        if args == [args[0], "--self-test"] {
            do {
                let dir = FileManager.default.temporaryDirectory.appendingPathComponent("sw-capture-test-\(UUID().uuidString)")
                try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: false)
                defer { try? FileManager.default.removeItem(at: dir) }
                let path = dir.appendingPathComponent("image.png").path
                guard let context = CGContext(data: nil, width: 2, height: 2, bitsPerComponent: 8,
                                              bytesPerRow: 8, space: CGColorSpaceCreateDeviceRGB(),
                                              bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue),
                      let image = context.makeImage() else { throw CaptureFailure("cannot create synthetic image") }
                let first = try pngData(image)
                guard let decoded = NSBitmapImageRep(data: first), decoded.pixelsWide == 2, decoded.pixelsHigh == 2 else {
                    throw CaptureFailure("synthetic PNG has invalid dimensions")
                }
                try writeNew(first, path: path)
                do { try writeNew(Data([4]), path: path); throw CaptureFailure("existing file was overwritten") }
                catch let e as CaptureFailure where e.description.hasPrefix("cannot create output:") { }
                guard try Data(contentsOf: URL(fileURLWithPath: path)) == first else {
                    throw CaptureFailure("original file changed")
                }
                print("synthetic PNG and exclusive output self-test passed; no screen capture")
                return
            } catch { fputs("self-test failed: \(error)\n", stderr); exit(1) }
        }
        let includeOffscreen = args.count == 5 && args[1] == "--include-offscreen"
        let offset = includeOffscreen ? 1 : 0
        guard args.count == 4 + offset, let id = UInt32(args[1 + offset]), id > 0,
              let pid = Int32(args[2 + offset]), pid > 0, args[3 + offset].hasPrefix("/") else {
            fputs("usage: helper [--include-offscreen] WINDOW_ID OWNER_PID /absolute/new-output.png\n", stderr)
            exit(2)
        }
        guard #available(macOS 14.0, *) else { fputs("macOS 14 or later required\n", stderr); exit(1) }
        // A bare CLI has not created AppKit's application context. Keep the
        // helper non-activating and initialise it before ScreenCaptureKit.
        let application = NSApplication.shared
        if application.activationPolicy() != .prohibited && !application.setActivationPolicy(.prohibited) {
            fputs("cannot set non-activating application policy\n", stderr)
            exit(1)
        }
        guard application.activationPolicy() == .prohibited else {
            fputs("non-activating application policy was not applied\n", stderr)
            exit(1)
        }
        do { try await capture(id: id, pid: pid, output: args[3 + offset], includeOffscreen: includeOffscreen) }
        catch {
            fputs("capture failed: \(error)\n", stderr)
            exit((error as? CaptureFailure)?.code ?? 1)
        }
    }
}
