// Snapper: a tiny app that owns the Screen Recording permission so scripted
// UI tests can screenshot abromux (via ScreenCaptureKit, so macOS doesn't
// re-prompt on every capture).
//   open -W -a Snapper.app --args --request              # ask macOS for permission
//   open -W -a Snapper.app --args --window <id> <out.png> # capture one window
import AppKit
import CoreGraphics
import Foundation
import ImageIO
import ScreenCaptureKit
import UniformTypeIdentifiers

func fail(_ msg: String) -> Never {
    FileHandle.standardError.write((msg + "\n").data(using: .utf8)!)
    exit(1)
}

let args = Array(CommandLine.arguments.dropFirst())
if args.first == "--request" {
    exit(CGRequestScreenCaptureAccess() ? 0 : 1)
}
guard args.count == 3, args[0] == "--window", let windowID = UInt32(args[1]) else {
    fail("usage: --window <id> <out.png>")
}
let out = URL(fileURLWithPath: args[2])
// Connects to the window server; without it captureImage trips
// CGS_REQUIRE_INIT on recent macOS.
_ = NSApplication.shared

Task {
    do {
        let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false)
        guard let window = content.windows.first(where: { $0.windowID == windowID }) else {
            fail("window \(windowID) not found")
        }
        let filter = SCContentFilter(desktopIndependentWindow: window)
        let config = SCStreamConfiguration()
        let scale = CGFloat(filter.pointPixelScale)
        config.width = Int(filter.contentRect.width * scale)
        config.height = Int(filter.contentRect.height * scale)
        config.showsCursor = false
        config.ignoreShadowsSingleWindow = true
        let image = try await SCScreenshotManager.captureImage(contentFilter: filter, configuration: config)
        guard let dest = CGImageDestinationCreateWithURL(out as CFURL, UTType.png.identifier as CFString, 1, nil) else {
            fail("cannot write \(out.path)")
        }
        CGImageDestinationAddImage(dest, image, nil)
        exit(CGImageDestinationFinalize(dest) ? 0 : 1)
    } catch {
        fail("capture failed: \(error)")
    }
}
RunLoop.main.run()
