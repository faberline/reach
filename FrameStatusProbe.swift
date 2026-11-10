import Foundation
import AppKit
import ScreenCaptureKit
import CoreMedia
import CoreGraphics

final class Receiver: NSObject, SCStreamOutput, @unchecked Sendable {
    let ready = DispatchSemaphore(value: 0)
    let lock = NSLock()
    var first: [String: Any]?
    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of outputType: SCStreamOutputType) {
        guard outputType == .screen else { return }
        let array = CMSampleBufferGetSampleAttachmentsArray(sampleBuffer, createIfNecessary: false)
        let attachments = (array as? [[SCStreamFrameInfo:Any]])?.first
        let status = (attachments?[.status] as? NSNumber)?.intValue
        let display = (attachments?[.displayTime] as? NSNumber)?.uint64Value
        let pts = CMSampleBufferGetPresentationTimeStamp(sampleBuffer)
        lock.lock()
        if first == nil {
            first = ["array_available": array != nil, "attachments_available": attachments != nil,
                "status": status as Any? ?? NSNull(), "display_time_available": display != nil,
                "pts_valid": pts.isValid, "buffer_available": CMSampleBufferGetImageBuffer(sampleBuffer) != nil]
            ready.signal()
        }
        lock.unlock()
    }
    func result() -> [String:Any]? { lock.lock(); defer { lock.unlock() }; return first }
}

@main struct Main {
    @MainActor static func main() async {
        do {
            let app = NSApplication.shared
            guard app.activationPolicy() == .prohibited || app.setActivationPolicy(.prohibited), app.activationPolicy() == .prohibited else { throw NSError(domain:"activation",code:1) }
            guard CommandLine.arguments.count == 3, let pid = Int32(CommandLine.arguments[1]), let wid = UInt32(CommandLine.arguments[2]), pid > 0, wid > 0, CGPreflightScreenCaptureAccess() else { throw NSError(domain:"args",code:1) }
            let content = try await SCShareableContent.excludingDesktopWindows(true,onScreenWindowsOnly:false)
            let matches = content.windows.filter { $0.windowID == wid && $0.owningApplication?.processID == pid }
            guard matches.count == 1, let window = matches.first else { throw NSError(domain:"window",code:1) }
            let filter = SCContentFilter(desktopIndependentWindow:window)
            let config = SCStreamConfiguration()
            let rect = filter.contentRect; let scale = Double(filter.pointPixelScale)
            guard scale.isFinite, scale > 0, rect.width > 0, rect.height > 0, rect.width*scale <= 8192, rect.height*scale <= 8192 else { throw NSError(domain:"geometry",code:1) }
            config.width=Int((rect.width*scale).rounded(.up)); config.height=Int((rect.height*scale).rounded(.up)); config.showsCursor=false
            config.minimumFrameInterval=CMTime(value:1,timescale:2); config.queueDepth=3
            let receiver = Receiver(); let stream = SCStream(filter:filter,configuration:config,delegate:nil)
            try stream.addStreamOutput(receiver,type:.screen,sampleHandlerQueue:DispatchQueue(label:"sw-frame-metadata"))
            try await stream.startCapture()
            let received = await withCheckedContinuation { continuation in
                DispatchQueue.global().async { continuation.resume(returning: receiver.ready.wait(timeout:.now()+3) == .success) }
            }
            try await stream.stopCapture()
            let afterContent = try await SCShareableContent.excludingDesktopWindows(true,onScreenWindowsOnly:false)
            let after = afterContent.windows.filter { $0.windowID == wid && $0.owningApplication?.processID == pid }
            guard after.count == 1, let afterWindow = after.first, afterWindow.frame == window.frame,
                  afterWindow.isOnScreen == window.isOnScreen else { throw NSError(domain:"window_changed",code:1) }
            let report:[String:Any] = ["schema":"cu-stream-status-v1","pid":pid,"window_id":wid,
                "onscreen_before":window.isOnScreen,"onscreen_after":afterWindow.isOnScreen,
                "received":received,"first":receiver.result() as Any? ?? NSNull(),"input_posted":false,
                "action_allowed":false,"scope":"stream_metadata_only"]
            print(String(decoding:try JSONSerialization.data(withJSONObject:report,options:[.sortedKeys]),as:UTF8.self))
        } catch { print("{\"status\":\"error\",\"input_posted\":false}");exit(1) }
    }
}
