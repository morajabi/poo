import AppKit
import SwiftUI

@main
struct PooApp: App {
    @State private var store = Store()
    @State private var updater = Updater()
    @State private var nav = Navigation()
    @AppStorage("showInMenuBar") private var showInMenuBar = true
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    var body: some Scene {
        MenuBarExtra(isInserted: $showInMenuBar) {
            MenuBarView()
                .environment(store)
                .environment(updater)
                .environment(nav)
        } label: {
            MenuBarLabel(store: store)
        }
        .menuBarExtraStyle(.window)

        Window("Poo", id: "main") {
            MainWindow()
                .environment(store)
                .environment(updater)
                .environment(nav)
                .frame(minWidth: 900, minHeight: 560)
                .onAppear { NSApp.setActivationPolicy(.regular) }
                .onDisappear { AppDelegate.updateActivationPolicy(windowOpen: false) }
        }
        .defaultSize(width: 1120, height: 740)
        .commands {
            CommandGroup(after: .appInfo) {
                Button("Check for Updates…") { updater.checkForUpdates() }
                    .disabled(!updater.enabled || !updater.canCheck)
                Button("Set Up Poo…") { delegate.show("onboarding", title: "Set Up Poo") }
            }
            CommandGroup(after: .textEditing) {
                Button("Find") { AppDelegate.focusToolbarSearch() }
                    .keyboardShortcut("f")
            }
        }

        Window("Set Up Poo", id: "onboarding") {
            OnboardingView()
                .environment(store)
        }
        .windowResizability(.contentSize)
        .windowStyle(.hiddenTitleBar)

        // `Poo --preview-menu` shows the menu bar panel in a window (screenshots, debugging).
        Window("Poo Menu", id: "menu-preview") {
            MenuBarView()
                .environment(store)
                .environment(updater)
                .environment(nav)
        }
        .windowResizability(.contentSize)
        .commandsRemoved()

        // `Poo --preview-confirm` shows the review sheet for the `stale` plan in a window (screenshots).
        Window("Poo Review", id: "confirm-preview") {
            CleanConfirmView(planID: "stale", free: nil)
                .environment(store)
        }
        .windowResizability(.contentSize)
        .commandsRemoved()

        // `Poo --preview-cleaning` runs the stale plan as a dry run (nothing is deleted) to show
        // the progress UI; `--preview-cleaning=fake` simulates it without calling the CLI.
        Window("Poo Cleaning", id: "cleaning-preview") {
            CleaningPreview()
                .environment(store)
        }
        .windowResizability(.contentSize)
        .commandsRemoved()

        Settings {
            SettingsView()
                .environment(store)
                .environment(updater)
                .frame(width: 720, height: 520)
        }
    }
}

/// Which section the main window shows; the menu bar can deep-link into it.
@Observable
final class Navigation {
    var section: Section? = .overview
    /// Browse filters live here so other views can deep-link into Browse.
    var browse = BrowseFilter()
    /// The review sheet the main window shows (a plan or a hand-picked selection).
    var review: Review?

    struct Review: Identifiable {
        let id = UUID()
        var planID: String?
        var free: UInt64?
        var items: [Item]?
    }

    func review(items: [Item]) {
        guard !items.isEmpty else { return }
        review = Review(items: items)
    }

    func review(plan: String, free: UInt64? = nil) {
        review = Review(planID: plan, free: free)
    }

    enum Section: Hashable {
        case overview
        case browse
        case projects
        case worktrees
        case backups
        case bucket(String)
    }

    /// Opens Browse narrowed to a bucket, group or status.
    func browse(bucket: String? = nil, group: String? = nil, statuses: Set<ItemStatus> = []) {
        browse.bucket = bucket
        browse.group = group
        browse.statuses = statuses
        browse.query = ""
        section = .browse
    }
}

struct BrowseFilter: Equatable {
    enum Grouping: String, CaseIterable, Identifiable {
        case type, project, status, none
        var id: String { rawValue }
        var label: String {
            switch self {
            case .type: return "Type"
            case .project: return "Project"
            case .status: return "Status"
            case .none: return "None"
            }
        }
    }

    enum Sorting: String, CaseIterable, Identifiable {
        case size, lastUsed, name
        var id: String { rawValue }
        var label: String {
            switch self {
            case .size: return "Size"
            case .lastUsed: return "Last used"
            case .name: return "Name"
            }
        }
    }

    var grouping: Grouping = .type
    var sorting: Sorting = .size
    var query = ""
    var statuses: Set<ItemStatus> = []
    var bucket: String?
    var group: String?
}

/// Launch flow and Dock/menu bar behavior. Works whether or not the menu bar icon exists.
final class AppDelegate: NSObject, NSApplicationDelegate {
    /// Set by the first SwiftUI view that appears; opens a window scene by id.
    var open: ((String) -> Void)?
    /// `--preview-section=…`, applied by the main window when it appears.
    var previewSection: String?
    static weak var shared: AppDelegate?

    override init() {
        super.init()
        Self.shared = self
    }

    func show(_ id: String, title: String) {
        if let open { open(id) } else { Self.openFromWindowMenu(title) }
    }

    static var showInMenuBar: Bool {
        let d = UserDefaults.standard
        return d.object(forKey: "showInMenuBar") == nil ? true : d.bool(forKey: "showInMenuBar")
    }

    static func updateActivationPolicy(windowOpen: Bool) {
        NSApp.setActivationPolicy(showInMenuBar && !windowOpen ? .accessory : .regular)
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        Self.updateActivationPolicy(windowOpen: false)
        // Without the menu bar icon nothing else triggers the launch flow, so open the main
        // window through the Window menu item SwiftUI made for it.
        if !Self.showInMenuBar {
            DispatchQueue.main.async { [weak self] in
                if self?.open == nil { Self.openFromWindowMenu(Onboarding.isDone ? "Poo" : "Set Up Poo") }
            }
        }
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows: Bool) -> Bool {
        if !hasVisibleWindows {
            if let open { open("main") } else { Self.openFromWindowMenu("Poo") }
        }
        return true
    }

    /// ⌘F: focus the toolbar search field of the key window (Browse).
    static func focusToolbarSearch() {
        guard let window = NSApp.keyWindow ?? NSApp.mainWindow else { return }
        if let item = window.toolbar?.items.compactMap({ $0 as? NSSearchToolbarItem }).first {
            item.beginSearchInteraction()
        } else if let field = findSearchField(in: window.contentView?.superview) {
            window.makeFirstResponder(field)
        }
    }

    private static func findSearchField(in view: NSView?) -> NSSearchField? {
        guard let view else { return nil }
        if let f = view as? NSSearchField { return f }
        for v in view.subviews { if let f = findSearchField(in: v) { return f } }
        return nil
    }

    static func openFromWindowMenu(_ title: String) {
        guard let items = NSApp.mainMenu?.items.flatMap({ $0.submenu?.items ?? [] }),
              let item = items.first(where: { $0.title == title }), let action = item.action else { return }
        NSApp.sendAction(action, to: item.target, from: item)
        NSApp.activate(ignoringOtherApps: true)
    }
}

/// Runs the launch flow once, from whichever view appears first.
enum LaunchFlow {
    @MainActor private static var ran = false

    @MainActor static func run(_ openWindow: OpenWindowAction) {
        AppDelegate.shared?.open = { id in openWindow(id: id); NSApp.activate(ignoringOtherApps: true) }
        guard !ran else { return }
        ran = true
        let args = CommandLine.arguments
        // Screenshot helpers: --dark, --preview-section=browse, --preview-settings=menubar.
        if args.contains("--dark") { NSApp.appearance = NSAppearance(named: .darkAqua) }
        if let tab = args.first(where: { $0.hasPrefix("--preview-settings=") })?.split(separator: "=").last {
            UserDefaults.standard.set(String(tab), forKey: "settingsTab")
        }
        if let sec = args.first(where: { $0.hasPrefix("--preview-section=") })?.split(separator: "=").last {
            AppDelegate.shared?.previewSection = String(sec)
        }
        if args.contains("--preview-menu") { openWindow(id: "menu-preview") }
        if args.contains("--preview-confirm") { openWindow(id: "confirm-preview") }
        if args.contains(where: { $0.hasPrefix("--preview-cleaning") }) { openWindow(id: "cleaning-preview") }
        if !Onboarding.isDone {
            openWindow(id: "onboarding")
        } else if args.contains("--window") || !AppDelegate.showInMenuBar {
            openWindow(id: "main")
        }
        NSApp.activate(ignoringOtherApps: true)
    }
}

struct MenuBarLabel: View {
    var store: Store
    /// In Settings: draw only, don't run the launch flow.
    var preview = false
    @Environment(\.openWindow) private var openWindow
    @Environment(\.openSettings) private var openSettings

    var body: some View {
        let cfg = store.menubar
        let warn = store.menubarWarns
        let text = store.report.map { cfg.text(for: $0) } ?? ""
        HStack(spacing: 3) {
            if cfg.shows(.icon) || text.isEmpty {
                Image(systemName: store.busy ? "wind" : warn ? "exclamationmark.triangle.fill" : "internaldrive")
            }
            if !text.isEmpty {
                Text(text)
                    .monospacedDigit()
                    .foregroundStyle(warn ? Color.orange : Color.primary)
            }
        }
        .accessibilityLabel(store.report.map { "Poo, \(Fmt.bytes($0.disk.free)) free" } ?? "Poo")
        .help(warn ? "Low on space" : "")
        .task {
            guard !preview else { return }
            LaunchFlow.run(openWindow)
            if CommandLine.arguments.contains(where: { $0.hasPrefix("--preview-settings=") }) {
                try? await Task.sleep(for: .milliseconds(300))
                openSettings()
            }
        }
    }
}
