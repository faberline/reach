import CoreGraphics
import Foundation

guard Array(CommandLine.arguments.dropFirst()) == ["--sample"] else {
    fputs("expected --sample\n", stderr); exit(2)
}
let session = CGSessionCopyCurrentDictionary() as? [String: Any]
let value: [String: Any] = ["status":"unknown", "action_allowed":false, "input_posted":false,
    "dictionary_available":session != nil,
    "on_console":session?[kCGSessionOnConsoleKey] as? Bool ?? NSNull(),
    "login_done":session?[kCGSessionLoginDoneKey] as? Bool ?? NSNull(),
    "note":"Session flags do not identify a session, prove isolation, show lock state, or authorize input."]
let data = try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
FileHandle.standardOutput.write(data); FileHandle.standardOutput.write(Data([10]))
