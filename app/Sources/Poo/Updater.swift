import AppKit
import CryptoKit
import Foundation
import Observation

/// Self-update from the same `latest.json` manifest the CLI and install script use.
/// The app bundle carries its own copy of the CLI, so updating the app updates both.
@MainActor
@Observable
final class Updater {
    var available: String?
    var checking = false
    var status: String?
    private var manifest: Manifest?

    struct Manifest: Decodable {
        var version: String
        var notes: String?
        var assets: [String: String]
        var sha256: [String: String]
    }

    static var manifestURL: URL {
        if let s = ProcessInfo.processInfo.environment["POO_RELEASES_URL"], let u = URL(string: s) { return u }
        return URL(string: "https://github.com/morajabi/poo/releases/latest/download/latest.json")!
    }

    var currentVersion: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0.0.0"
    }

    init() {
        Task {
            try? await Task.sleep(for: .seconds(20))
            while true {
                await check(userInitiated: false)
                try? await Task.sleep(for: .seconds(6 * 3600))
            }
        }
    }

    func check(userInitiated: Bool) async {
        checking = true
        defer { checking = false }
        do {
            let (data, _) = try await URLSession.shared.data(from: Self.manifestURL)
            let m = try JSONDecoder().decode(Manifest.self, from: data)
            manifest = m
            if Self.isNewer(m.version, than: currentVersion) {
                available = m.version
                status = m.notes
            } else {
                available = nil
                if userInitiated { status = "You have the latest version." }
            }
        } catch {
            if userInitiated { status = "Could not check: \(error.localizedDescription)" }
        }
    }

    nonisolated static func isNewer(_ a: String, than b: String) -> Bool {
        let parse: (String) -> [Int] = { $0.trimmingCharacters(in: CharacterSet(charactersIn: "v")).split(whereSeparator: { $0 == "." || $0 == "-" }).map { Int($0) ?? 0 } }
        let x = parse(a), y = parse(b)
        for i in 0..<max(x.count, y.count) {
            let l = i < x.count ? x[i] : 0, r = i < y.count ? y[i] : 0
            if l != r { return l > r }
        }
        return false
    }

    func install() async {
        guard let m = manifest, let urlString = m.assets["app-macos"], let url = URL(string: urlString),
              let expected = m.sha256["app-macos"] else {
            status = "No app download in the release."
            return
        }
        status = "Downloading \(m.version)…"
        do {
            let (tmp, _) = try await URLSession.shared.download(from: url)
            let data = try Data(contentsOf: tmp)
            let digest = SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
            guard digest.lowercased() == expected.lowercased() else {
                status = "Download checksum did not match; update cancelled."
                return
            }
            let work = FileManager.default.temporaryDirectory.appendingPathComponent("poo-update-\(UUID().uuidString)")
            try FileManager.default.createDirectory(at: work, withIntermediateDirectories: true)
            let zip = work.appendingPathComponent("Poo.zip")
            try data.write(to: zip)
            try run("/usr/bin/ditto", ["-x", "-k", zip.path, work.path])
            let newApp = work.appendingPathComponent("Poo.app")
            guard FileManager.default.fileExists(atPath: newApp.path) else {
                status = "The download did not contain Poo.app."
                return
            }
            let current = Bundle.main.bundleURL
            var trashed: NSURL?
            try FileManager.default.trashItem(at: current, resultingItemURL: &trashed)
            try FileManager.default.moveItem(at: newApp, to: current)
            status = "Restarting…"
            let p = Process()
            p.executableURL = URL(fileURLWithPath: "/usr/bin/open")
            p.arguments = ["-n", current.path]
            try p.run()
            NSApp.terminate(nil)
        } catch {
            status = "Update failed: \(error.localizedDescription)"
        }
    }

    private func run(_ tool: String, _ args: [String]) throws {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: tool)
        p.arguments = args
        try p.run()
        p.waitUntilExit()
        if p.terminationStatus != 0 {
            throw NSError(domain: "poo", code: Int(p.terminationStatus), userInfo: [NSLocalizedDescriptionKey: "\(tool) failed"])
        }
    }
}
