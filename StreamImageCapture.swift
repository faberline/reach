import Foundation
import AppKit
import ScreenCaptureKit
import CoreMedia
import CoreGraphics
import CoreImage
import Darwin

// Diagnostic only: these rectangles are not calibrated input coordinates.
func rectDiagnostic(_ value: Any?) -> [String:Double]? {
    guard let dict=value as? NSDictionary, CFGetTypeID(dict) == CFDictionaryGetTypeID(),
          let rect=CGRect(dictionaryRepresentation: unsafeBitCast(dict,to:CFDictionary.self)),
          rect.origin.x.isFinite, rect.origin.y.isFinite, rect.width.isFinite, rect.height.isFinite,
          rect.width > 0, rect.height > 0 else { return nil }
    return ["x":rect.origin.x,"y":rect.origin.y,"width":rect.width,"height":rect.height]
}

func scaleDiagnostic(_ value: Any?) -> Double? {
    guard let n=value as? NSNumber, CFGetTypeID(n) == CFNumberGetTypeID(),
          n.doubleValue.isFinite, n.doubleValue > 0 else { return nil }
    return n.doubleValue
}

// This reports timing only. It is not proof of app content freshness.
func sampleAgeMilliseconds(received: CMTime, sampled: CMTime) -> Double? {
    guard received.isNumeric, sampled.isNumeric else { return nil }
    let seconds=CMTimeSubtract(received,sampled).seconds
    guard seconds.isFinite, seconds >= 0 else { return nil }
    return seconds * 1000
}

func writeNew(_ data: Data, path: String) throws {
    let fd=open(path,O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW,0o600)
    guard fd >= 0 else { throw NSError(domain:"file_create",code:1) }
    var success=false
    defer { close(fd); if !success { unlink(path) } }
    try data.withUnsafeBytes { bytes in
        var offset=0
        while offset < bytes.count {
            let n=Darwin.write(fd,bytes.baseAddress!.advanced(by:offset),bytes.count-offset)
            if n < 0 && errno == EINTR { continue }
            guard n > 0 else { throw NSError(domain:"file_write",code:1) }
            offset += n
        }
    }
    guard fsync(fd)==0 else { throw NSError(domain:"file_sync",code:1) }
    success=true
}

final class Receiver: NSObject, SCStreamOutput, @unchecked Sendable {
    let ready = DispatchSemaphore(value: 0)
    let lock = NSLock()
    var first: [String: Any]?
    var firstSample: CMSampleBuffer?
    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of outputType: SCStreamOutputType) {
        guard outputType == .screen else { return }
        let array = CMSampleBufferGetSampleAttachmentsArray(sampleBuffer, createIfNecessary: false)
        let attachments = (array as? [[SCStreamFrameInfo:Any]])?.first
        let status = (attachments?[.status] as? NSNumber)?.intValue
        let display = (attachments?[.displayTime] as? NSNumber)?.uint64Value
        let pts = CMSampleBufferGetPresentationTimeStamp(sampleBuffer)
        let host = CMClockGetHostTimeClock()
        let receivedHost = CMClockGetTime(host)
        let streamClock = stream.synchronizationClock
        let sampleHost = streamClock.map { CMSyncConvertTime(pts,from:$0,to:host) }
        let ageMs = sampleHost.flatMap { sampleAgeMilliseconds(received:receivedHost,sampled:$0) }
        // Both values are in mach_absolute_time units, not wall-clock time.
        let sampleTicks = ageMs != nil ? sampleHost.map { CMClockConvertHostTimeToSystemUnits($0) } : nil
        let receiptTicks = ageMs != nil ? CMClockConvertHostTimeToSystemUnits(receivedHost) : nil
        lock.lock()
        if first == nil {
            firstSample = sampleBuffer
            first = ["array_available": array != nil, "attachments_available": attachments != nil,
                "status": status as Any? ?? NSNull(), "display_time_available": display != nil,
                "pts_valid": pts.isValid, "buffer_available": CMSampleBufferGetImageBuffer(sampleBuffer) != nil,
                "content_rect_points":rectDiagnostic(attachments?[.contentRect]) as Any? ?? NSNull(),
                "bounding_rect_points":rectDiagnostic(attachments?[.boundingRect]) as Any? ?? NSNull(),
                "content_scale":scaleDiagnostic(attachments?[.contentScale]) as Any? ?? NSNull(),
                "scale_factor":scaleDiagnostic(attachments?[.scaleFactor]) as Any? ?? NSNull(),
                "stream_clock_available":streamClock != nil,
                "sample_age_at_receipt_ms":ageMs as Any? ?? NSNull(),
                "sample_host_ticks":sampleTicks as Any? ?? NSNull(),
                "receipt_host_ticks":receiptTicks as Any? ?? NSNull()]
            ready.signal()
        }
        lock.unlock()
    }
    func result() -> [String:Any]? { lock.lock(); defer { lock.unlock() }; return first }
    func sample() -> CMSampleBuffer? { lock.lock(); defer { lock.unlock() }; return firstSample }
}

@main struct Main {
    @MainActor static func main() async {
        do {
            if CommandLine.arguments == [CommandLine.arguments[0], "--self-test"] {
                let rect=CGRect(x:2,y:3,width:10,height:20)
                let dictionary=rect.dictionaryRepresentation
                guard rectDiagnostic(dictionary)?["width"] == 10,
                      rectDiagnostic("not a rectangle") == nil,
                      scaleDiagnostic(NSNumber(value:2)) == 2,
                      scaleDiagnostic(NSNumber(value:true)) == nil,
                      scaleDiagnostic(NSNumber(value:Double.nan)) == nil,
                      sampleAgeMilliseconds(received:CMTime(seconds:2,preferredTimescale:1000),sampled:CMTime(seconds:1,preferredTimescale:1000)) == 1000,
                      sampleAgeMilliseconds(received:CMTime(seconds:1,preferredTimescale:1000),sampled:CMTime(seconds:2,preferredTimescale:1000)) == nil,
                      sampleAgeMilliseconds(received:.invalid,sampled:.zero) == nil else { throw NSError(domain:"geometry_test",code:1) }
                print("{\"status\":\"self_test_passed\",\"live_ui_access\":false}")
                return
            }
            let app = NSApplication.shared
            guard app.activationPolicy() == .prohibited || app.setActivationPolicy(.prohibited), app.activationPolicy() == .prohibited else { throw NSError(domain:"activation",code:1) }
            guard CommandLine.arguments.count == 4, let pid = Int32(CommandLine.arguments[1]), let wid = UInt32(CommandLine.arguments[2]), pid > 0, wid > 0, CommandLine.arguments[3].hasPrefix("/"), CGPreflightScreenCaptureAccess() else { throw NSError(domain:"args",code:1) }
            let content = try await SCShareableContent.excludingDesktopWindows(true,onScreenWindowsOnly:false)
            let matches = content.windows.filter { $0.windowID == wid && $0.owningApplication?.processID == pid }
            guard matches.count == 1, let window = matches.first else { throw NSError(domain:"window",code:1) }
            let filter = SCContentFilter(desktopIndependentWindow:window)
            let config = SCStreamConfiguration()
            let rect = filter.contentRect; let scale = Double(filter.pointPixelScale)
            guard scale.isFinite, scale > 0, rect.width.isFinite, rect.height.isFinite,
                  rect.width > 0, rect.height > 0, rect.width*scale <= 8192, rect.height*scale <= 8192,
                  window.frame.origin.x.isFinite, window.frame.origin.y.isFinite,
                  window.frame.width.isFinite, window.frame.height.isFinite else { throw NSError(domain:"geometry",code:1) }
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
            var report:[String:Any] = ["schema":"cu-stream-status-v1","pid":pid,"window_id":wid,
                "onscreen_before":window.isOnScreen,"onscreen_after":afterWindow.isOnScreen,
                "received":received,"first":receiver.result() as Any? ?? NSNull(),"input_posted":false,
                "action_allowed":false,"scope":"stream_image_same_sample","outcome":"incomplete",
                "filter_content_rect_points":["x":rect.origin.x,"y":rect.origin.y,"width":rect.width,"height":rect.height],
                "filter_point_pixel_scale":scale,
                "window_frame_points":["x":window.frame.origin.x,"y":window.frame.origin.y,
                                       "width":window.frame.width,"height":window.frame.height]]
            guard received, let metadata=receiver.result(), metadata["status"] as? Int == 0,
                  metadata["pts_valid"] as? Bool == true, metadata["display_time_available"] as? Bool == true,
                  let sample=receiver.sample(), let pixel=CMSampleBufferGetImageBuffer(sample) else {
                print(String(decoding:try JSONSerialization.data(withJSONObject:report,options:[.sortedKeys]),as:UTF8.self))
                return
            }
            let image=CIImage(cvPixelBuffer:pixel)
            let context=CIContext(options:nil)
            guard let cg=context.createCGImage(image,from:image.extent), cg.width==config.width, cg.height==config.height,
                  let png=NSBitmapImageRep(cgImage:cg).representation(using:.png,properties:[:]) else { throw NSError(domain:"image",code:1) }
            try writeNew(png,path:CommandLine.arguments[3])
            report["outcome"]="captured"; report["image_width"]=cg.width; report["image_height"]=cg.height
            report["image_path"]=CommandLine.arguments[3]
            print(String(decoding:try JSONSerialization.data(withJSONObject:report,options:[.sortedKeys]),as:UTF8.self))
        } catch {
            let e=error as NSError
            fputs("stream image failed: \(e.domain) \(e.code)\n",stderr)
            print("{\"status\":\"error\",\"input_posted\":false}");exit(1)
        }
    }
}
