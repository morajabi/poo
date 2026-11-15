import AppKit
import SwiftUI

@main
struct PooApp: App {
    @State private var store = Store()
    @State private var updater = Updater()
    @State private var nav = Navigation()

    var body: some Scene {
        MenuBarExtra {
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
                .frame(minWidth: 960, minHeight: 600)
                .onAppear { NSApp.setActivationPolicy(.regular) }
                .onDisappear { NSApp.setActivationPolicy(.accessory) }
        }
        .defaultSize(width: 1180, height: 760)

        // `Poo --preview-menu` shows the menu bar panel in a window (screenshots, debugging).
        Window("Poo Menu", id: "menu-preview") {
            MenuBarView()
                .environment(store)
                .environment(updater)
                .environment(nav)
        }
        .windowResizability(.contentSize)

        // `Poo --preview-confirm` shows the review sheet for the `stale` plan in a window (screenshots).
        Window("Poo Review", id: "confirm-preview") {
            CleanConfirmView(planID: "stale", free: nil)
                .environment(store)
        }
        .windowResizability(.contentSize)

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

struct MenuBarLabel: View {
    var store: Store
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        HStack(spacing: 3) {
            Image(systemName: store.scanning ? "arrow.triangle.2.circlepath" : "internaldrive")
            if store.lowDisk {
                Image(systemName: "exclamationmark.circle")
                    .help("Free space is below the low-disk guard's threshold")
            }
            if let r = store.report {
                Text(Fmt.bytes(r.disk.free))
                    .monospacedDigit()
            }
        }
        .task {
            NSApp.setActivationPolicy(.accessory)
            // `Poo --window` (and first launch) opens the main window right away.
            let firstLaunch = !UserDefaults.standard.bool(forKey: "launchedBefore")
            if CommandLine.arguments.contains("--preview-menu") {
                openWindow(id: "menu-preview")
                NSApp.activate(ignoringOtherApps: true)
            }
            if CommandLine.arguments.contains("--preview-confirm") {
                openWindow(id: "confirm-preview")
                NSApp.activate(ignoringOtherApps: true)
            }
            if CommandLine.arguments.contains("--window") || firstLaunch {
                UserDefaults.standard.set(true, forKey: "launchedBefore")
                openWindow(id: "main")
                NSApp.activate(ignoringOtherApps: true)
            }
        }
    }
}
