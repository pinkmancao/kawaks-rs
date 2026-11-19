import CoreGraphics
import Darwin
import Foundation

func hasVisibleWindow(ownedBy pid: Int32) -> Bool {
    // Owner PID and bounds are available even when window titles are restricted.
    let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
    guard let windows = CGWindowListCopyWindowInfo(options, kCGNullWindowID)
        as? [[String: Any]] else {
        return false
    }
    return windows.contains { window in
        guard window[kCGWindowOwnerPID as String] as? Int == Int(pid),
              window[kCGWindowLayer as String] as? Int == 0,
              let bounds = window[kCGWindowBounds as String] as? [String: Any],
              let rect = CGRect(dictionaryRepresentation: bounds as CFDictionary) else {
            return false
        }
        return rect.width > 0 && rect.height > 0
    }
}

func checkWindow(at executable: URL) throws -> Bool {
    let launcher = Process()
    launcher.executableURL = executable
    launcher.arguments = ["--gui"]
    var environment = ProcessInfo.processInfo.environment
    environment["SDL_VIDEODRIVER"] = "cocoa"
    launcher.environment = environment
    try launcher.run()
    defer {
        if launcher.isRunning {
            // Bound cleanup even if initialization stalls or event processing stops.
            Darwin.kill(launcher.processIdentifier, SIGKILL)
        }
        launcher.waitUntilExit()
    }

    let deadline = ProcessInfo.processInfo.systemUptime + 30
    var visibleOnPreviousPoll = false
    while ProcessInfo.processInfo.systemUptime < deadline {
        guard launcher.isRunning else {
            print("SDL2 launcher exited before window verification (status \(launcher.terminationStatus)).")
            return false
        }
        let visible = hasVisibleWindow(ownedBy: launcher.processIdentifier)
        if visible && visibleOnPreviousPoll {
            print("Verified visible Cocoa launcher window for PID \(launcher.processIdentifier).")
            return true
        }
        visibleOnPreviousPoll = visible
        Thread.sleep(forTimeInterval: 0.2)
    }

    print("No visible Cocoa launcher window after 30 seconds. Check the runner's WindowServer session and the SDL2 startup log.")
    return false
}

guard CommandLine.arguments.count == 2 else {
    print("Usage: swift tools/check-macos-window.swift path/to/kawaks-rs")
    exit(EXIT_FAILURE)
}

do {
    let executable = URL(fileURLWithPath: CommandLine.arguments[1]).standardizedFileURL
    exit(try checkWindow(at: executable) ? EXIT_SUCCESS : EXIT_FAILURE)
} catch {
    print("Could not start SDL2 launcher: \(error)")
    exit(EXIT_FAILURE)
}
