import CoreGraphics
import Foundation

let session = CGSessionCopyCurrentDictionary() as? [String: Any]
var result: [String: Any] = ["dictionary_available":session != nil, "input_posted":false, "action_allowed":false]
result["on_console"] = session?[kCGSessionOnConsoleKey] as? Bool ?? NSNull()
result["login_done"] = session?[kCGSessionLoginDoneKey] as? Bool ?? NSNull()
result["screen_locked_undocumented"] = session?["CGSSessionScreenIsLocked"] as? Bool ?? NSNull()
let data = try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys])
FileHandle.standardOutput.write(data)
FileHandle.standardOutput.write(Data([10]))
