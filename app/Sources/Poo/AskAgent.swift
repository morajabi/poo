import SwiftUI

/// Menu items that hand poo's data about `target` to the user's coding agent, so it can dig
/// into what a worktree, project or folder really is before anything gets cleaned.
/// Targets: an item id, `project:<path>`, `bucket:<id>` or `overview`.
struct AskAgentItems: View {
    @Environment(Store.self) private var store
    @Environment(\.openSettings) private var openSettings
    var target: String

    var body: some View {
        let list = store.launchers
        let available = list?.launchers.filter(\.available) ?? []
        Section("Ask your agent") {
            ForEach(available) { l in
                Button("Ask \(l.name)") { Task { await store.ask(target, with: l.id) } }
            }
            Button("Copy prompt") { Task { await store.copyPrompt(target) } }
            if list?.skill_installed == false {
                Button("Set up agent…") {
                    UserDefaults.standard.set("agents", forKey: "settingsTab")
                    NSApp.activate(ignoringOtherApps: true)
                    openSettings()
                }
            }
        }
    }
}

/// A compact "Ask your agent" menu button.
struct AskAgentButton: View {
    @Environment(Store.self) private var store
    var target: String
    var title: String? = nil

    var body: some View {
        Menu {
            AskAgentItems(target: target)
        } label: {
            if let title {
                Label(title, systemImage: "sparkles").labelStyle(.titleAndIcon)
            } else {
                Image(systemName: "sparkles")
            }
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(title == nil ? .hidden : .visible)
        .fixedSize()
        .help("Ask your coding agent to look into this with Poo's data")
        .task { await store.loadLaunchers() }
    }
}

/// A transient confirmation shown at the bottom of a window ("Prompt copied").
struct ToastOverlay: View {
    @Environment(Store.self) private var store

    var body: some View {
        if let t = store.toast {
            Label(t, systemImage: "checkmark.circle.fill")
                .font(.callout)
                .padding(.horizontal, 12)
                .padding(.vertical, 7)
                .glassBackground(in: Capsule())
                .padding(.bottom, 56)
                .transition(.opacity.combined(with: .move(edge: .bottom)))
        }
    }
}
