import AppKit
import ServiceManagement
import SwiftUI
import UserNotifications

enum Onboarding {
    static let doneKey = "onboardingDone"

    /// People who used Poo before setup existed count as set up.
    static var isDone: Bool {
        let d = UserDefaults.standard
        if ProcessInfo.processInfo.arguments.contains("--onboarding") { return false }
        return d.bool(forKey: doneKey) || d.bool(forKey: "launchedBefore")
    }

    static func markDone() {
        UserDefaults.standard.set(true, forKey: doneKey)
        UserDefaults.standard.set(true, forKey: "launchedBefore")
    }

    static var isBundled: Bool { Bundle.main.bundleIdentifier != nil && Bundle.main.bundleURL.pathExtension == "app" }
}

/// First-run setup: a few quick steps, nothing scanned or scheduled until the user says so.
struct OnboardingView: View {
    @Environment(Store.self) private var store
    @Environment(\.openWindow) private var openWindow
    @Environment(\.dismissWindow) private var dismissWindow
    @AppStorage("showInMenuBar") private var showInMenuBar = true

    enum Step: Int, CaseIterable {
        case welcome, folders, home, permissions, routines, agents
        var title: String {
            switch self {
            case .welcome: return "Welcome"
            case .folders: return "Dev folders"
            case .home: return "Where Poo lives"
            case .permissions: return "Permissions"
            case .routines: return "Routine cleanups"
            case .agents: return "Agents"
            }
        }
    }

    @State private var step: Step = {
        let arg = ProcessInfo.processInfo.arguments.first { $0.hasPrefix("--onboarding-step=") }
        return arg.flatMap { Int($0.split(separator: "=").last ?? "") }.flatMap(Step.init(rawValue:)) ?? .welcome
    }()
    @State private var fullDiskAccess: Bool?
    @State private var notifications: UNAuthorizationStatus?
    @State private var nightly = true
    @State private var guardOn = true
    @State private var loginItem = SMAppService.mainApp.status == .enabled
    @State private var loginError: String?
    @State private var working = false

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 6) {
                ForEach(Step.allCases, id: \.self) { s in
                    Capsule()
                        .fill(s.rawValue <= step.rawValue ? Color.accentColor : Color.secondary.opacity(0.25))
                        .frame(height: 3)
                }
            }
            .padding(.horizontal, 24)
            .padding(.top, 14)
            VStack(alignment: .leading, spacing: 14) {
                Text(step == .welcome ? "Set up Poo" : step.title)
                    .font(.title2.weight(.semibold))
                content
                Spacer(minLength: 0)
            }
            .padding(24)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            Divider()
            HStack {
                if step != .welcome {
                    Button("Back") { go(-1) }
                }
                Spacer()
                if step != .welcome && step != .agents {
                    Button("Skip") { go(1, apply: false) }
                }
                Button(step == .agents ? "Finish & scan" : "Continue") { go(1) }
                    .keyboardShortcut(.defaultAction)
                    .disabled(working)
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 12)
        }
        .frame(width: 580, height: 480)
        .task(id: step) { await load() }
        .onAppear { LaunchFlow.run(openWindow) }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            Task { await checkPermissions() }
        }
    }

    @ViewBuilder
    private var content: some View {
        switch step {
        case .welcome: welcome
        case .folders: folders
        case .home: home
        case .permissions: permissions
        case .routines: routines
        case .agents: agents
        }
    }

    // MARK: Steps

    private var welcome: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Poo finds build output, caches, simulators and old worktrees your dev tools leave behind, and frees the space without touching anything you're using.")
                .fixedSize(horizontal: false, vertical: true)
            VStack(alignment: .leading, spacing: 8) {
                bullet("magnifyingglass", "Scans only your dev folders and known tool caches.")
                bullet("checkmark.shield", "Never deletes on its own. You review every clean, and risky items are skipped.")
                bullet("archivebox", "Backs up small config files and uncommitted work before removing a worktree.")
                bullet("sparkles", "Hands its findings to your coding agent when you want a second opinion.")
            }
            Text("Takes about a minute. You can change everything later in Settings.")
                .font(.callout).foregroundStyle(.secondary)
        }
    }

    private var folders: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Poo looks for projects in these folders. Remove any you don't want scanned.")
                .foregroundStyle(.secondary)
            if let roots = store.roots {
                List {
                    ForEach(roots.roots, id: \.self) { r in
                        HStack {
                            Image(systemName: "folder").foregroundStyle(.secondary)
                            Text(Fmt.tilde(r))
                            Spacer()
                            Text(projectCount(r) == 1 ? "1 project" : "\(projectCount(r)) projects").font(.caption).foregroundStyle(.secondary)
                                .opacity(store.report == nil ? 0 : 1)
                            Button { Task { await store.configure(["roots", "rm", r]) } } label: { Image(systemName: "minus.circle") }
                                .buttonStyle(.borderless)
                                .help("Don't scan this folder")
                        }
                    }
                }
                .listStyle(.bordered)
                .frame(minHeight: 140)
                if roots.detected {
                    Text("Detected automatically. Changing the list pins it.").font(.caption).foregroundStyle(.secondary)
                }
            } else {
                ProgressView().controlSize(.small)
            }
            Button("Add folder…") { pickFolder() }
        }
    }

    private var home: some View {
        VStack(alignment: .leading, spacing: 12) {
            Picker("", selection: $showInMenuBar) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Menu bar and window")
                    Text("Free space at a glance in the menu bar. Open the window when you want to clean.")
                        .font(.caption).foregroundStyle(.secondary)
                }
                .tag(true)
                VStack(alignment: .leading, spacing: 2) {
                    Text("Window only")
                    Text("A regular app in the Dock, no menu bar icon.")
                        .font(.caption).foregroundStyle(.secondary)
                }
                .tag(false)
            }
            .pickerStyle(.radioGroup)
            .labelsHidden()
            Divider()
            Toggle(isOn: Binding(get: { loginItem }, set: { setLogin($0) })) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Open at login")
                    Text("Keeps the menu bar number and the low-disk guard running.").font(.caption).foregroundStyle(.secondary)
                }
            }
            if let loginError {
                Text(loginError).font(.caption).foregroundStyle(.secondary)
            }
        }
    }

    private var permissions: some View {
        VStack(alignment: .leading, spacing: 14) {
            permissionRow(
                title: "Full Disk Access",
                detail: "Lets Poo include browser, Mail, Messages and app-container caches without macOS blocking or prompting. Without it Poo skips those folders.",
                granted: fullDiskAccess,
                button: "Open Privacy Settings"
            ) {
                NSWorkspace.shared.open(URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")!)
            }
            if Onboarding.isBundled, fullDiskAccess != true {
                HStack(spacing: 10) {
                    AppTile()
                    Text("Drag Poo into the list, or add it with +. macOS may ask you to reopen Poo afterwards.")
                        .font(.caption).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .padding(.leading, 28)
            }
            permissionRow(
                title: "Notifications",
                detail: "A short note when a routine cleanup or the low-disk guard frees space.",
                granted: notifications.map { $0 == .authorized || $0 == .provisional },
                button: notifications == .denied ? "Open Notification Settings" : "Allow"
            ) {
                if notifications == .denied {
                    NSWorkspace.shared.open(URL(string: "x-apple.systempreferences:com.apple.Notifications-Settings.extension")!)
                } else {
                    requestNotifications()
                }
            }
            .disabled(!Onboarding.isBundled)
            Text("Without Full Disk Access, macOS asks once the first time Poo looks in Desktop, Documents or Downloads.")
                .font(.caption).foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var routines: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Poo can keep things tidy on its own. Routine cleanups rescan first and only clean items without warnings: nothing in use, nothing that needs a look.")
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            Toggle(isOn: $nightly) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Nightly cleanup")
                    Text("Every night at 03:30, clean what's not in use.").font(.caption).foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            Toggle(isOn: $guardOn) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Low-disk guard")
                    Text("When free space drops below 20 GB, clean what's not in use until 50 GB is free.").font(.caption).foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            Text("Applied when you press Continue. Change them later in Settings › Automation.")
                .font(.caption).foregroundStyle(.secondary)
        }
        .toggleStyle(.switch)
    }

    private var agents: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Poo's static analysis can't know everything. Let your coding agent look deeper: it gets Poo's data and can check worktrees and files with you.")
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            let detected = store.integrations.filter(\.detected)
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Skill and MCP server")
                    Text(detected.isEmpty ? "No coding agents found." : detected.map(\.name).joined(separator: ", "))
                        .font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                if !detected.isEmpty && detected.allSatisfy({ $0.skill_current && $0.mcp_registered }) {
                    Label("Installed", systemImage: "checkmark.circle.fill").foregroundStyle(.green).font(.callout)
                } else {
                    Button("Install skill + MCP") { Task { await store.configure(["integration", "install", "all"]) } }
                        .disabled(detected.isEmpty)
                }
            }
            if let h = store.hooks, !h.targets.isEmpty {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Clean up after agents (optional)")
                    ForEach(h.targets.filter { $0.detected ?? true }) { t in
                        Toggle(t.name, isOn: Binding(get: { t.installed == true }, set: { on in
                            Task { await store.setHook(t.id, installed: on) }
                        }))
                        .toggleStyle(.checkbox)
                    }
                    Text("When an agent session ends in a worktree whose work landed in main, Poo cleans that worktree's build output. Never the code.")
                        .font(.caption).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            HStack {
                Button("Copy setup prompt") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(AgentSettings.setupPrompt, forType: .string)
                    store.flash("Copied")
                }
                Text(store.toast ?? "Paste it into your agent to let it tune Poo for your projects.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
    }

    // MARK: Pieces

    private func bullet(_ symbol: String, _ text: String) -> some View {
        Label {
            Text(text).fixedSize(horizontal: false, vertical: true)
        } icon: {
            Image(systemName: symbol).foregroundStyle(Color.accentColor).frame(width: 20)
        }
    }

    private func permissionRow(title: String, detail: String, granted: Bool?, button: String, action: @escaping () -> Void) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: granted == true ? "checkmark.circle.fill" : "circle")
                .foregroundStyle(granted == true ? Color.green : Color.secondary)
                .font(.title3)
            VStack(alignment: .leading, spacing: 2) {
                Text(title).fontWeight(.medium)
                Text(detail).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
            Spacer()
            if granted != true {
                Button(button, action: action)
            } else {
                Text("Allowed").font(.callout).foregroundStyle(.secondary)
            }
        }
    }

    private func projectCount(_ root: String) -> Int {
        store.report?.projects.filter { $0.path.hasPrefix(root) }.count ?? 0
    }

    // MARK: Actions

    private func load() async {
        switch step {
        case .folders: store.roots = try? await PooCLI.json(RootsInfo.self, ["roots"])
        case .permissions: await checkPermissions()
        case .routines: await store.loadSchedules()
        case .agents:
            store.integrations = (try? await PooCLI.json(IntegrationStatus.self, ["integration", "status"]))?.targets ?? []
            await store.loadHooks()
        default: break
        }
    }

    private func checkPermissions() async {
        fullDiskAccess = (try? await PooCLI.json(Catalog.self, ["catalog"]))?.full_disk_access
        if Onboarding.isBundled {
            notifications = await UNUserNotificationCenter.current().notificationSettings().authorizationStatus
        }
    }

    private func requestNotifications() {
        Task {
            _ = try? await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound])
            await checkPermissions()
        }
    }

    private func setLogin(_ on: Bool) {
        do {
            if on { try SMAppService.mainApp.register() } else { try SMAppService.mainApp.unregister() }
            loginItem = on
            loginError = nil
        } catch {
            loginItem = SMAppService.mainApp.status == .enabled
            loginError = Onboarding.isBundled ? error.localizedDescription : "Available when Poo runs from /Applications."
        }
    }

    private func pickFolder() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = true
        panel.prompt = "Add"
        if panel.runModal() == .OK {
            Task { for url in panel.urls { await store.configure(["roots", "add", url.path]) } }
        }
    }

    private func go(_ delta: Int, apply: Bool = true) {
        if delta > 0 && step == .routines && apply {
            working = true
            Task {
                await applyRoutines()
                working = false
                step = .agents
            }
            return
        }
        if delta > 0 && step == .agents {
            finish()
            return
        }
        if let next = Step(rawValue: step.rawValue + delta) { step = next }
    }

    private func applyRoutines() async {
        let existing = Set(store.schedules.map(\.id))
        if nightly && !existing.contains("nightly") {
            await store.configure(["schedule", "add", "nightly"])
        }
        if guardOn && !existing.contains("guard") {
            await store.configure(["schedule", "add", "guard", "--preset", "guard", "--when-free-below", "20G", "--target-free", "50G"])
        }
        await store.loadSchedules()
    }

    private func finish() {
        Onboarding.markDone()
        openWindow(id: "main")
        NSApp.activate(ignoringOtherApps: true)
        dismissWindow(id: "onboarding")
        store.startGuard()
        Task { await store.scan() }
    }
}

/// The app's icon and name, draggable into the Full Disk Access list.
struct AppTile: View {
    var body: some View {
        let url = Bundle.main.bundleURL
        HStack(spacing: 6) {
            Image(nsImage: NSWorkspace.shared.icon(forFile: url.path))
                .resizable()
                .frame(width: 28, height: 28)
            Text(FileManager.default.displayName(atPath: url.path)).font(.callout)
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 5)
        .background(.background.secondary, in: RoundedRectangle(cornerRadius: 7))
        .overlay(RoundedRectangle(cornerRadius: 7).strokeBorder(.separator))
        .draggable(url)
        .help("Drag into System Settings › Privacy & Security › Full Disk Access")
    }
}
