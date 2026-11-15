import Foundation

/// Runs the `poo` CLI. The app never deletes anything itself: every action goes
/// through the CLI so the safety floor, guards and history stay in one place.
struct PooCLI {
    struct Failure: LocalizedError {
        var kind: String
        var message: String
        var hint: String?
        var errorDescription: String? { hint.map { "\(message)\n\($0)" } ?? message }
    }

    static var binary: URL? {
        if let bundled = Bundle.main.url(forResource: "poo", withExtension: nil, subdirectory: "bin"),
           FileManager.default.isExecutableFile(atPath: bundled.path) {
            return bundled
        }
        let candidates = [
            ProcessInfo.processInfo.environment["POO_BIN"],
            "\(NSHomeDirectory())/.local/bin/poo",
            "/opt/homebrew/bin/poo",
            "/usr/local/bin/poo",
        ].compactMap { $0 }
        return candidates.map { URL(fileURLWithPath: $0) }.first { FileManager.default.isExecutableFile(atPath: $0.path) }
    }

    /// The cached report the CLI writes, for an instant first frame.
    static var reportURL: URL {
        let env = ProcessInfo.processInfo.environment
        if let s = env["POO_STATE_DIR"] { return URL(fileURLWithPath: s).appendingPathComponent("report.json") }
        if let h = env["POO_HOME"] { return URL(fileURLWithPath: h).appendingPathComponent("state/report.json") }
        if let x = env["XDG_STATE_HOME"] { return URL(fileURLWithPath: x).appendingPathComponent("poo/report.json") }
        return URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent(".local/state/poo/report.json")
    }

    static func cachedReport() -> Report? {
        guard let data = try? Data(contentsOf: reportURL) else { return nil }
        return try? JSONDecoder().decode(Report.self, from: data)
    }

    /// Runs `poo <args> --json` and returns stdout. Calls `onStderrLine` for each stderr line.
    static func run(_ args: [String], onStderrLine: (@Sendable (String) -> Void)? = nil, onStart: (@Sendable (Process) -> Void)? = nil) async throws -> Data {
        guard let bin = binary else {
            throw Failure(kind: "missing", message: "The poo command-line tool was not found.", hint: "Reinstall Poo, or install the CLI with the install script.")
        }
        return try await withCheckedThrowingContinuation { cont in
            let p = Process()
            p.executableURL = bin
            p.arguments = args + ["--json"]
            let out = Pipe()
            let err = Pipe()
            p.standardOutput = out
            p.standardError = err
            p.standardInput = FileHandle.nullDevice
            let collector = StderrCollector(onLine: onStderrLine)
            err.fileHandleForReading.readabilityHandler = { h in
                let d = h.availableData
                if !d.isEmpty { collector.append(d) }
            }
            let stdout = DataBox()
            out.fileHandleForReading.readabilityHandler = { h in
                stdout.append(h.availableData)
            }
            p.terminationHandler = { proc in
                out.fileHandleForReading.readabilityHandler = nil
                err.fileHandleForReading.readabilityHandler = nil
                stdout.append(out.fileHandleForReading.readDataToEndOfFile())
                let result = stdout.data
                collector.append(err.fileHandleForReading.readDataToEndOfFile())
                if proc.terminationStatus == 0 {
                    cont.resume(returning: result)
                } else {
                    cont.resume(throwing: collector.failure(status: proc.terminationStatus))
                }
            }
            do {
                try p.run()
                onStart?(p)
            } catch { cont.resume(throwing: error) }
        }
    }

    static func json<T: Decodable>(_ type: T.Type, _ args: [String]) async throws -> T {
        let data = try await run(args)
        return try JSONDecoder().decode(T.self, from: data)
    }
}

/// Collects stderr, forwards progress lines and remembers the last JSON error.
final class StderrCollector: @unchecked Sendable {
    private var buffer = Data()
    private var lastError: PooCLI.Failure?
    /// Recent plain stderr lines, for failures that aren't JSON (argument errors).
    private var plain: [String] = []
    private let lock = NSLock()
    private let onLine: (@Sendable (String) -> Void)?

    init(onLine: (@Sendable (String) -> Void)?) { self.onLine = onLine }

    func append(_ d: Data) {
        lock.lock()
        buffer.append(d)
        var lines: [String] = []
        while let nl = buffer.firstIndex(of: 0x0A) {
            let line = String(decoding: buffer[buffer.startIndex..<nl], as: UTF8.self)
            buffer.removeSubrange(buffer.startIndex...nl)
            lines.append(line)
        }
        lock.unlock()
        for line in lines {
            if let data = line.data(using: .utf8),
               let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
               let e = obj["error"] as? [String: Any] {
                lock.lock()
                lastError = PooCLI.Failure(kind: e["kind"] as? String ?? "error", message: e["message"] as? String ?? line, hint: e["hint"] as? String)
                lock.unlock()
            } else {
                if !line.hasPrefix("{"), !line.trimmingCharacters(in: .whitespaces).isEmpty {
                    lock.lock()
                    plain.append(line)
                    if plain.count > 6 { plain.removeFirst() }
                    lock.unlock()
                }
                onLine?(line)
            }
        }
    }

    func failure(status: Int32) -> PooCLI.Failure {
        lock.lock(); defer { lock.unlock() }
        if let lastError { return lastError }
        let text = plain.first { $0.hasPrefix("error:") } ?? plain.last
        return PooCLI.Failure(kind: text?.contains("unexpected argument") == true ? "usage" : "error",
                              message: text ?? "poo exited with status \(status)", hint: nil)
    }
}

final class DataBox: @unchecked Sendable {
    private var buffer = Data()
    private let lock = NSLock()
    func append(_ d: Data) { lock.lock(); buffer.append(d); lock.unlock() }
    var data: Data { lock.lock(); defer { lock.unlock() }; return buffer }
}
