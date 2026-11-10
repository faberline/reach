import Foundation
import Vision
import ImageIO
import CryptoKit

// Offline-only recognition. It neither connects to a window nor sends input.
enum OCRFailure: Error { case invalidArguments, invalidFile, badImage, changedFile, tooManyResults }
func readBounded(_ url: URL) throws -> Data {
    let handle = try FileHandle(forReadingFrom: url)
    defer { try? handle.close() }
    var result = Data()
    while true {
        let part = try handle.read(upToCount: 1_048_576) ?? Data()
        if part.isEmpty { break }
        guard part.count <= 40_000_000 - result.count else { throw OCRFailure.invalidFile }
        result.append(part)
    }
    return result
}
func run() throws -> [String: Any] {
    let args = CommandLine.arguments
    guard args.count == 2, args[1].hasPrefix("/") else { throw OCRFailure.invalidArguments }
    let url = URL(fileURLWithPath: args[1])
    let values = try url.resourceValues(forKeys: [.isRegularFileKey, .fileSizeKey, .isSymbolicLinkKey])
    guard values.isRegularFile == true, values.isSymbolicLink != true,
          let size = values.fileSize, size > 0, size <= 40_000_000 else { throw OCRFailure.invalidFile }
    let data = try readBounded(url)
    guard data.count == size, let source = CGImageSourceCreateWithData(data as CFData, nil),
          CGImageSourceGetCount(source) == 1, let image = CGImageSourceCreateImageAtIndex(source, 0, nil),
          image.width > 0, image.height > 0, image.width <= 8192, image.height <= 8192 else { throw OCRFailure.badImage }
    let digest = SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
    let request = VNRecognizeTextRequest()
    request.recognitionLevel = .accurate
    request.recognitionLanguages = ["zh-Hant", "en-US"]
    request.usesLanguageCorrection = false
    try VNImageRequestHandler(cgImage: image, options: [:]).perform([request])
    let observations = request.results ?? []
    guard observations.count <= 1000 else { throw OCRFailure.tooManyResults }
    let texts: [[String: Any]] = observations.compactMap { observation in
        guard let candidate = observation.topCandidates(1).first else { return nil }
        let rect = observation.boundingBox
        return ["text": candidate.string, "confidence_uncalibrated": candidate.confidence,
                "box_normalized_bottom_left": ["x": rect.origin.x, "y": rect.origin.y, "width": rect.width, "height": rect.height]]
    }
    let after = try readBounded(url)
    guard after == data else { throw OCRFailure.changedFile }
    return ["status": "review_required", "action_allowed": false, "image_sha256": digest,
            "image_width": image.width, "image_height": image.height, "items": texts,
            "note": "OCR can omit or misread text. Boxes are image coordinates, not validated window coordinates. Never authorize input."]
}
do {
    let result = try run()
    let bytes = try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys])
    FileHandle.standardOutput.write(bytes); FileHandle.standardOutput.write(Data([10]))
    exit(0)
} catch {
    let bytes = try! JSONSerialization.data(withJSONObject: ["status":"error", "action_allowed":false, "reason":String(describing:error)], options: [.sortedKeys])
    FileHandle.standardOutput.write(bytes); FileHandle.standardOutput.write(Data([10])); exit(1)
}
