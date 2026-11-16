import AppKit
import Foundation

/// Help › Report a Problem…: a prefilled GitHub issue with what's useful to debug
/// (versions, `poo doctor`), home paths shortened to ~. Shown for review and editing
/// first; nothing is sent until the person opens the issue in their browser.
enum ProblemReport {
    static let repo = "morajabi/poo"

    @MainActor
    static func start() {
        Task {
            let doctor = (try? await PooCLI.run(["doctor", "--text"])).flatMap { String(data: $0, encoding: .utf8) } ?? "(poo doctor did not run)"
            present(body(doctor: doctor))
        }
    }

    static func body(doctor: String) -> String {
        let info = Bundle.main.infoDictionary ?? [:]
        let app = (info["CFBundleShortVersionString"] as? String) ?? "dev"
        let os = ProcessInfo.processInfo.operatingSystemVersionString
        var arch = "x86_64"
        #if arch(arm64)
        arch = "arm64"
        #endif
        let text = """
        ### What happened

        <!-- What did you do, what did you expect, what happened instead? Screenshots help. -->


        ### Environment

        - Poo \(app), macOS \(os), \(arch)

        <details><summary>poo doctor</summary>

        ```
        \(doctor.trimmingCharacters(in: .whitespacesAndNewlines))
        ```

        </details>
        """
        return scrub(text)
    }

    /// Home folder → ~, and the account name wherever else it appears.
    static func scrub(_ s: String) -> String {
        var out = s.replacingOccurrences(of: NSHomeDirectory(), with: "~")
        // Whole words only, so a short account name never eats parts of other words.
        let user = NSUserName()
        if user.count >= 3, let re = try? NSRegularExpression(pattern: "\\b\(NSRegularExpression.escapedPattern(for: user))\\b") {
            out = re.stringByReplacingMatches(in: out, range: NSRange(out.startIndex..., in: out), withTemplate: "<user>")
        }
        return out
    }

    @MainActor
    private static func present(_ body: String) {
        let alert = NSAlert()
        alert.messageText = "Report a Problem"
        alert.informativeText = "This opens a new issue on GitHub with the text below. Edit anything you'd rather not share; nothing is sent until you submit it there."
        let scroll = NSTextView.scrollableTextView()
        scroll.frame = NSRect(x: 0, y: 0, width: 520, height: 260)
        if let tv = scroll.documentView as? NSTextView {
            tv.string = body
            tv.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
            tv.isRichText = false
            tv.textContainerInset = NSSize(width: 4, height: 6)
        }
        alert.accessoryView = scroll
        alert.addButton(withTitle: "Open GitHub Issue")
        alert.addButton(withTitle: "Copy")
        alert.addButton(withTitle: "Cancel")
        NSApp.activate()
        let response = alert.runModal()
        let text = (scroll.documentView as? NSTextView)?.string ?? body
        switch response {
        case .alertFirstButtonReturn:
            NSWorkspace.shared.open(issueURL(body: text))
        case .alertSecondButtonReturn:
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(text, forType: .string)
        default:
            break
        }
    }

    /// GitHub caps prefilled URLs around 8 KB: long reports are cut, with a note to paste the rest.
    static func issueURL(body: String) -> URL {
        var b = body
        let limit = 6000
        if b.utf8.count > limit {
            b = String(b.prefix(limit)) + "\n\n(cut to fit; Poo copied the full report, paste it here)"
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(body, forType: .string)
        }
        var c = URLComponents(string: "https://github.com/\(repo)/issues/new")!
        c.queryItems = [URLQueryItem(name: "body", value: b)]
        return c.url!
    }
}
