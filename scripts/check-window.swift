// Exit 0 if a process with the given PID owns an on-screen window, printing
// the window bounds; exit 1 otherwise. Used by CI to check that the GUI
// actually opens a window. Usage: swift scripts/check-window.swift <pid>
import CoreGraphics
import Foundation

guard CommandLine.arguments.count == 2, let pid = Int(CommandLine.arguments[1]) else {
    FileHandle.standardError.write("usage: check-window.swift <pid>\n".data(using: .utf8)!)
    exit(2)
}

let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
let mine = windows.filter { ($0[kCGWindowOwnerPID as String] as? Int) == pid }
for w in mine {
    let owner = w[kCGWindowOwnerName as String] as? String ?? "?"
    let bounds = w[kCGWindowBounds as String] as? [String: Any] ?? [:]
    print("window owned by \(owner) (pid \(pid)): \(bounds)")
}
exit(mine.isEmpty ? 1 : 0)
