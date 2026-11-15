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

    enum Section: Hashable {
        case overview
        case bucket(String)
        case projects
        case worktrees
        case history
    }
}

struct MenuBarLabel: View {
    var store: Store
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        HStack(spacing: 3) {
            Image(systemName: store.scanning ? "arrow.triangle.2.circlepath" : "internaldrive")
            if let r = store.report {
                Text(Fmt.bytes(r.disk.free))
                    .monospacedDigit()
            }
        }
        .task {
            NSApp.setActivationPolicy(.accessory)
            // `Poo --window` (and first launch) opens the main window right away.
            let firstLaunch = !UserDefaults.standard.bool(forKey: "launchedBefore")
            if CommandLine.arguments.contains("--window") || firstLaunch {
                UserDefaults.standard.set(true, forKey: "launchedBefore")
                openWindow(id: "main")
                NSApp.activate(ignoringOtherApps: true)
            }
        }
    }
}
