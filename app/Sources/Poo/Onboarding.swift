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

    /// Reading the TCC folder works only with Full Disk Access, and never prompts.
    static var hasFullDiskAccess: Bool {
        (try? FileManager.default.contentsOfDirectory(atPath: NSHomeDirectory() + "/Library/Application Support/com.apple.TCC")) != nil
    }
}

/// First run, from install to the first clean. Each step does one thing, reacts to what
/// the user does (folders found, Full Disk Access switched on), and the first scan runs
/// underneath from the first second, so the last step shows real numbers.
struct OnboardingView: View {
    @Environment(Store.self) private var store
    @Environment(Navigation.self) private var nav
    @Environment(\.openWindow) private var openWindow
    @Environment(\.dismissWindow) private var dismissWindow
    @AppStorage("showInMenuBar") private var showInMenuBar = true
    /// Survives "Quit & Reopen" after granting Full Disk Access.
    @AppStorage("onboardingStep") private var savedStep = 0

    enum Step: Int, CaseIterable {
        case welcome, folders, access, routines, agents, ready
        var title: String {
            switch self {
            case .welcome: return "Welcome to Poo"
            case .folders: return "Where your code lives"
            case .access: return "See the whole disk"
            case .routines: return "Keep it tidy"
            case .agents: return "Terminal and agents"
            case .ready: return "You're set"
            }
        }
    }

    @State private var step: Step = .welcome
    @State private var fullDiskAccess = Onboarding.hasFullDiskAccess
    @State private var openedSettings = false
    @State private var justGranted = false
    @State private var nightly = true
    @State private var guardOn = true
    @State private var notify = true
    @State private var loginItem = SMAppService.mainApp.status == .enabled
    @State private var loginError: String?
    @State private var working = false
    @State private var cliState = CommandLineTool.State.notInstalled

    var body: some View {
        VStack(spacing: 0) {
            if step != .welcome {
                progressDots
                    .padding(.top, 16)
            }
            Group {
                switch step {
                case .welcome: welcome
                case .folders: folders
                case .access: access
                case .routines: routines
                case .agents: agents
                case .ready: ready
                }
            }
            .padding(.horizontal, 32)
            .padding(.vertical, 20)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .transition(.asymmetric(insertion: .move(edge: .trailing).combined(with: .opacity), removal: .opacity))
            .id(step)
            if step != .welcome && step != .ready {
                Divider()
                footer
                    .padding(.horizontal, 20)
                    .padding(.vertical, 12)
            }
        }
        .frame(width: 600, height: 520)
        .animation(.smooth(duration: 0.3), value: step)
        .onAppear {
            LaunchFlow.run(openWindow)
            if let arg = ProcessInfo.processInfo.arguments.first(where: { $0.hasPrefix("--onboarding-step=") }),
               let n = Int(arg.split(separator: "=").last ?? ""), let s = Step(rawValue: n) {
                step = s
            } else if let s = Step(rawValue: savedStep) {
                step = s
            }
            // The first look starts now, with whatever Poo can read, so later steps have numbers.
            if store.report == nil, !store.scanning { Task { await store.scan() } }
        }
        .onChange(of: step) { _, s in savedStep = s.rawValue }
        .task(id: step) { await load() }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            refreshAccess()
        }
    }

    // MARK: Chrome

    private var progressDots: some View {
        HStack(spacing: 6) {
            ForEach(Step.allCases.dropFirst(), id: \.self) { s in
                Capsule()
                    .fill(s.rawValue <= step.rawValue ? Color.accentColor : Color.secondary.opacity(0.22))
                    .frame(width: s == step ? 22 : 7, height: 7)
            }
        }
        .animation(.snappy, value: step)
        .accessibilityLabel("Step \(step.rawValue) of \(Step.allCases.count - 1)")
    }

    private var footer: some View {
        HStack {
            if step != .ready {
                Button("Back") { go(-1) }
                    .keyboardShortcut(.cancelAction)
            }
            Spacer()
            if store.scanning {
                // One quiet line: the first look keeps going while you set things up.
                HStack(spacing: 6) {
                    ProgressView().controlSize(.mini)
                    Text(store.progress.map { "Looking… \(Fmt.bytes($0.bytes))" } ?? "Looking…")
                        .monospacedDigit()
                        .contentTransition(.numericText())
                }
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .fixedSize()
                .animation(.default, value: store.progress?.bytes)
            }
            Spacer()
            if step != .ready {
                Button(step == .access && !fullDiskAccess ? "Skip for Now" : "Continue") { go(1) }
                    .keyboardShortcut(.defaultAction)
                    .disabled(working)
            }
        }
    }

    private func header(_ subtitle: String) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(step.title).font(.title.weight(.semibold))
            Text(subtitle).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
        }
        .padding(.bottom, 8)
    }

    // MARK: Welcome

    private var welcome: some View {
        VStack(spacing: 18) {
            Spacer(minLength: 0)
            Image(nsImage: NSApp.applicationIconImage)
                .resizable()
                .frame(width: 96, height: 96)
            VStack(spacing: 6) {
                Text("Welcome to Poo").font(.largeTitle.weight(.semibold))
                Text("Finds the space your dev tools leave behind and frees it, without touching anything you use.")
                    .font(.title3)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 420)
            }
            VStack(alignment: .leading, spacing: 10) {
                bullet("hammer", "Build output, caches, simulators and old worktrees")
                bullet("checkmark.shield", "Nothing goes without your review")
                bullet("archivebox", "Uncommitted work is backed up first")
            }
            .padding(.top, 4)
            Spacer(minLength: 0)
            Button { go(1) } label: {
                Text("Get Started").frame(width: 200)
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.extraLarge)
            .keyboardShortcut(.defaultAction)
            Text("About a minute. Change anything later in Settings.")
                .font(.caption)
                .foregroundStyle(.tertiary)
        }
        .frame(maxWidth: .infinity)
    }

    // MARK: Folders

    private var folders: some View {
        VStack(alignment: .leading, spacing: 12) {
            header("Poo looks for projects in these folders. It found them on its own; add any it missed.")
            if let roots = store.roots {
                VStack(spacing: 0) {
                    ForEach(Array(roots.roots.enumerated()), id: \.element) { idx, r in
                        if idx > 0 { Divider().padding(.leading, 40) }
                        HStack(spacing: 10) {
                            Image(systemName: "folder.fill").foregroundStyle(Color.accentColor).frame(width: 20)
                            Text(Fmt.tilde(r))
                            Spacer()
                            Group {
                                if let n = projectCount(r) {
                                    Text(n == 1 ? "1 project" : "\(n) projects")
                                        .contentTransition(.numericText())
                                } else {
                                    ProgressView().controlSize(.mini)
                                }
                            }
                            .font(.callout).foregroundStyle(.secondary)
                            Button { Task { await store.configure(["roots", "rm", r]); await reloadRoots() } } label: {
                                Image(systemName: "minus.circle.fill").foregroundStyle(.secondary)
                            }
                            .buttonStyle(.borderless)
                            .help("Don't look in this folder")
                        }
                        .padding(.horizontal, 12)
                        .padding(.vertical, 9)
                        .contextMenu {
                            Button("Reveal in Finder") { NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: r)]) }
                            Button("Remove") { Task { await store.configure(["roots", "rm", r]); await reloadRoots() } }
                        }
                    }
                    if roots.roots.isEmpty {
                        Text("No dev folders found yet. Add the folder that holds your projects.")
                            .foregroundStyle(.secondary)
                            .padding(12)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
                .card()
                .animation(.default, value: store.report?.projects.count)
            } else {
                ProgressView().controlSize(.small)
            }
            Button { pickFolder() } label: { Label("Add Folder…", systemImage: "plus") }
            Spacer(minLength: 0)
            Text("Poo also checks the caches your tools keep in your Library, wherever your code is.")
                .font(.caption).foregroundStyle(.secondary)
        }
    }

    // MARK: Full Disk Access

    private var access: some View {
        VStack(alignment: .leading, spacing: 14) {
            header("macOS keeps some folders private: Downloads, Docker's disk, other apps' caches. One switch lets Poo see them, so it never has to ask folder by folder.")
            if fullDiskAccess {
                HStack(spacing: 14) {
                    Image(systemName: "checkmark.circle.fill")
                        .font(.system(size: 36))
                        .foregroundStyle(.green)
                        .symbolEffect(.bounce, value: justGranted)
                    VStack(alignment: .leading, spacing: 3) {
                        Text("Full Disk Access is on").font(.title3.weight(.semibold))
                        Text(justGranted ? "Poo is taking another look with everything in view." : "Poo can see the whole disk.")
                            .foregroundStyle(.secondary)
                    }
                }
                .padding(16)
                .frame(maxWidth: .infinity, alignment: .leading)
                .card()
                .transition(.scale(scale: 0.95).combined(with: .opacity))
            } else {
                VStack(alignment: .leading, spacing: 12) {
                    numbered(1, "Open Privacy & Security") {
                        Button("Open System Settings") {
                            openedSettings = true
                            NSWorkspace.shared.open(URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")!)
                        }
                        .buttonStyle(.borderedProminent)
                    }
                    numbered(2, "Turn on Poo in Full Disk Access") {
                        if Onboarding.isBundled {
                            HStack(spacing: 8) {
                                Text("Not in the list? Drag it in:").font(.caption).foregroundStyle(.secondary)
                                AppTile()
                            }
                        }
                    }
                    numbered(3, "Come back here") {
                        HStack(spacing: 6) {
                            ProgressView().controlSize(.mini)
                            Text("Poo notices on its own.").font(.caption).foregroundStyle(.secondary)
                        }
                    }
                }
                .padding(16)
                .frame(maxWidth: .infinity, alignment: .leading)
                .card()
                if openedSettings {
                    HStack(spacing: 8) {
                        Image(systemName: "arrow.clockwise.circle").foregroundStyle(.secondary)
                        Text("Turned it on and nothing changed? macOS applies it after Poo reopens.")
                            .font(.callout).foregroundStyle(.secondary)
                        Spacer()
                        Button("Quit & Reopen") { relaunch() }
                    }
                    .transition(.opacity)
                }
            }
            Spacer(minLength: 0)
            Label("Poo only reads to measure. It never sends your files anywhere.", systemImage: "lock")
                .font(.caption).foregroundStyle(.secondary)
        }
        .animation(.smooth, value: fullDiskAccess)
        .animation(.smooth, value: openedSettings)
        .task(id: step) {
            // Watch for the switch while this step is up: no relaunch or button needed.
            while step == .access && !Task.isCancelled {
                refreshAccess()
                try? await Task.sleep(for: .seconds(1))
            }
        }
    }

    private func numbered<C: View>(_ n: Int, _ title: String, @ViewBuilder _ content: () -> C) -> some View {
        HStack(alignment: .top, spacing: 12) {
            Text("\(n)")
                .font(.callout.weight(.semibold))
                .foregroundStyle(.white)
                .frame(width: 22, height: 22)
                .background(Circle().fill(Color.accentColor))
            VStack(alignment: .leading, spacing: 6) {
                Text(title).fontWeight(.medium)
                content()
            }
        }
    }

    // MARK: Routines

    private var routines: some View {
        VStack(alignment: .leading, spacing: 12) {
            header("Poo can clean on its own, only what nothing is using. You can watch every run in Settings.")
            VStack(spacing: 0) {
                toggleRow("moon.stars", "Nightly cleanup", "Every night at 03:30", $nightly)
                Divider().padding(.leading, 44)
                toggleRow("gauge.with.dots.needle.0percent", "Low-disk guard", "Below 5 GB free, clean until 20 GB are free", $guardOn)
                Divider().padding(.leading, 44)
                toggleRow("bell", "Tell me when it cleans", "A short notification with what was freed", $notify)
                    .disabled(!nightly && !guardOn)
            }
            .card()
            VStack(spacing: 0) {
                toggleRow("menubar.rectangle", "Show in the menu bar", "Free space at a glance, one click to clean", $showInMenuBar)
                Divider().padding(.leading, 44)
                toggleRow("power", "Open at login", "Keeps the menu bar and the guard running",
                          Binding(get: { loginItem }, set: { setLogin($0) }))
            }
            .card()
            if let loginError {
                Text(loginError).font(.caption).foregroundStyle(.secondary)
            }
            Spacer(minLength: 0)
        }
    }

    private func toggleRow(_ symbol: String, _ title: String, _ detail: String, _ on: Binding<Bool>) -> some View {
        Toggle(isOn: on) {
            HStack(spacing: 12) {
                Image(systemName: symbol).foregroundStyle(Color.accentColor).frame(width: 20)
                VStack(alignment: .leading, spacing: 1) {
                    Text(title)
                    Text(detail).font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
            }
        }
        .toggleStyle(.switch)
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }

    // MARK: Agents

    private var agents: some View {
        VStack(alignment: .leading, spacing: 12) {
            header("Use Poo from Terminal, and give your coding agent its tools so it can check your disk and clean with you. Optional.")
            let detected = store.integrations.filter(\.detected)
            VStack(spacing: 0) {
                cliRow
                Divider().padding(.leading, 44)
                HStack(spacing: 12) {
                    Image(systemName: "sparkles").foregroundStyle(Color.accentColor).frame(width: 20)
                    VStack(alignment: .leading, spacing: 1) {
                        Text("Skill and MCP server")
                        Text(detected.isEmpty ? "No coding agents found" : detected.map(\.name).joined(separator: ", "))
                            .font(.caption).foregroundStyle(.secondary)
                    }
                    Spacer()
                    if !detected.isEmpty && detected.allSatisfy({ $0.skill_current && $0.mcp_registered }) {
                        Label("Installed", systemImage: "checkmark.circle.fill").foregroundStyle(.green).font(.callout)
                    } else {
                        Button("Install") {
                            working = true
                            Task {
                                await store.configure(["integration", "install", "all"])
                                store.integrations = (try? await PooCLI.json(IntegrationStatus.self, ["integration", "status"]))?.targets ?? []
                                working = false
                            }
                        }
                        .disabled(detected.isEmpty || working)
                    }
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 10)
                if let h = store.hooks, !h.targets.filter({ $0.detected ?? true }).isEmpty {
                    Divider().padding(.leading, 44)
                    VStack(alignment: .leading, spacing: 6) {
                        HStack(spacing: 12) {
                            Image(systemName: "arrow.triangle.branch").foregroundStyle(Color.accentColor).frame(width: 20)
                            VStack(alignment: .leading, spacing: 1) {
                                Text("Clean up after agents")
                                Text("When a session ends in a worktree whose work landed, clean its build output. Never code.")
                                    .font(.caption).foregroundStyle(.secondary)
                                    .fixedSize(horizontal: false, vertical: true)
                            }
                        }
                        HStack(spacing: 14) {
                            ForEach(h.targets.filter { $0.detected ?? true }) { t in
                                Toggle(t.name, isOn: Binding(get: { t.installed == true }, set: { on in
                                    Task { await store.setHook(t.id, installed: on) }
                                }))
                                .toggleStyle(.checkbox)
                            }
                        }
                        .padding(.leading, 32)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 10)
                }
            }
            .card()
            HStack {
                Button("Copy Setup Prompt") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(AgentSettings.setupPrompt, forType: .string)
                    store.flash("Copied")
                }
                Text(store.toast ?? "Paste it into your agent to tune Poo to your projects.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Spacer(minLength: 0)
        }
    }

    private var cliRow: some View {
        HStack(spacing: 12) {
            Image(systemName: "terminal").foregroundStyle(Color.accentColor).frame(width: 20)
            VStack(alignment: .leading, spacing: 1) {
                Text("Command-line tool")
                Group {
                    switch cliState {
                    case .linked:
                        if CommandLineTool.onPath {
                            Text("Type poo in Terminal. Updates with the app.")
                        } else {
                            Text("Add ~/.local/bin to your PATH to type poo.")
                        }
                    case .separate(let p): Text("Installed at \(Fmt.tilde(p))")
                    case .notInstalled: Text("poo in Terminal: the same engine, plus a full-screen view")
                    case .unavailable: Text("Not bundled in this build")
                    }
                }
                .font(.caption).foregroundStyle(.secondary)
                .textSelection(.enabled)
            }
            Spacer()
            switch cliState {
            case .linked where !CommandLineTool.onPath:
                Button("Copy PATH Line") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(CommandLineTool.pathLine, forType: .string)
                    store.flash("Copied. Paste it in Terminal.")
                }
            case .linked, .separate:
                Label("Installed", systemImage: "checkmark.circle.fill").foregroundStyle(.green).font(.callout)
            case .notInstalled:
                Button("Install") {
                    if let err = CommandLineTool.linkNow() { store.flash(err) }
                    cliState = CommandLineTool.state
                }
            case .unavailable:
                EmptyView()
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .onAppear { cliState = CommandLineTool.state }
    }

    // MARK: Ready

    @ViewBuilder
    private var ready: some View {
        if let r = store.report, !store.scanning || store.progress == nil {
            let safe = r.plan("stale")?.bytes ?? 0
            let areas = AreaSummary.notInUse(r).prefix(4)
            VStack(spacing: 16) {
                Spacer(minLength: 0)
                VStack(spacing: 4) {
                    Text(safe > 0 ? Fmt.bytes(safe) : "All clean")
                        .font(.system(size: 44, weight: .semibold))
                        .monospacedDigit()
                        .contentTransition(.numericText())
                    Text(safe > 0 ? "you can free right now, nothing in use" : "Nothing needs cleaning. Poo keeps watching.")
                        .font(.title3).foregroundStyle(.secondary)
                }
                if !areas.isEmpty {
                    VStack(spacing: 0) {
                        ForEach(Array(areas.enumerated()), id: \.element.id) { idx, a in
                            if idx > 0 { Divider().padding(.leading, 46) }
                            HStack(spacing: 10) {
                                AreaIcon(symbol: a.bucket.symbol, color: a.bucket.swiftColor, size: 24)
                                Text(a.title).lineLimit(1)
                                Spacer()
                                Text(Fmt.bytes(a.reclaim)).monospacedDigit().foregroundStyle(.secondary)
                            }
                            .padding(.horizontal, 12)
                            .padding(.vertical, 7)
                        }
                    }
                    .card()
                    .frame(maxWidth: 400)
                }
                Spacer(minLength: 0)
                HStack(spacing: 12) {
                    Button("Done") { finish(review: false) }
                        .controlSize(.large)
                    if safe > 0 {
                        Button("Review & Clean…") { finish(review: true) }
                            .buttonStyle(.borderedProminent)
                            .controlSize(.large)
                            .keyboardShortcut(.defaultAction)
                    } else {
                        Button("Open Poo") { finish(review: false, open: true) }
                            .buttonStyle(.borderedProminent)
                            .controlSize(.large)
                            .keyboardShortcut(.defaultAction)
                    }
                }
                Label(showInMenuBar ? "Poo lives in your menu bar, at the top right." : "Open Poo from the Dock or Spotlight any time.",
                      systemImage: showInMenuBar ? "arrow.up.right" : "dock.rectangle")
                    .font(.caption).foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity)
            .transition(.opacity)
        } else {
            ScanProgressView()
        }
    }

    // MARK: Pieces

    private func bullet(_ symbol: String, _ text: String) -> some View {
        Label {
            Text(text)
        } icon: {
            Image(systemName: symbol).foregroundStyle(Color.accentColor).frame(width: 22)
        }
        .font(.body)
    }

    /// nil while the first scan hasn't counted projects yet.
    private func projectCount(_ root: String) -> Int? {
        guard let r = store.report else { return nil }
        return r.projects.filter { $0.path.hasPrefix(root) }.count
    }

    // MARK: Actions

    private func load() async {
        switch step {
        case .folders: await reloadRoots()
        case .agents:
            store.integrations = (try? await PooCLI.json(IntegrationStatus.self, ["integration", "status"]))?.targets ?? []
            await store.loadHooks()
        case .routines: await store.loadSchedules()
        default: break
        }
    }

    private func reloadRoots() async {
        store.roots = try? await PooCLI.json(RootsInfo.self, ["roots"])
    }

    private func refreshAccess() {
        let now = Onboarding.hasFullDiskAccess
        guard now != fullDiskAccess else { return }
        fullDiskAccess = now
        if now {
            justGranted = true
            // Folders that were hidden are readable now: walk everything once.
            Task {
                while store.scanning { try? await Task.sleep(for: .milliseconds(300)) }
                await store.scan(full: true)
            }
        }
    }

    private func relaunch() {
        savedStep = Step.access.rawValue
        let path = Bundle.main.bundlePath
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/bin/sh")
        p.arguments = ["-c", "sleep 1; open \"\(path)\""]
        try? p.run()
        NSApp.terminate(nil)
    }

    private func setLogin(_ on: Bool) {
        do {
            if on { try SMAppService.mainApp.register() } else { try SMAppService.mainApp.unregister() }
            loginItem = on
            loginError = nil
        } catch {
            loginItem = SMAppService.mainApp.status == .enabled
            loginError = Onboarding.isBundled ? error.localizedDescription : "Available when Poo runs from Applications."
        }
    }

    private func pickFolder() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = true
        panel.prompt = "Add"
        if panel.runModal() == .OK {
            Task {
                for url in panel.urls { await store.configure(["roots", "add", url.path]) }
                await reloadRoots()
                await store.scan(full: true)
            }
        }
    }

    private func go(_ delta: Int) {
        if delta > 0 && step == .routines {
            working = true
            Task {
                await applyRoutines()
                working = false
                step = .agents
            }
            return
        }
        if let next = Step(rawValue: step.rawValue + delta) { step = next }
    }

    private func applyRoutines() async {
        AppDelegate.updateActivationPolicy(windowOpen: true)
        let existing = Set(store.schedules.map(\.id))
        if nightly && !existing.contains("nightly") {
            await store.configure(["schedule", "add", "nightly"])
        }
        if guardOn && !existing.contains("guard") {
            await store.configure(["schedule", "add", "guard", "--preset", "guard", "--when-free-below", "5G", "--target-free", "20G"])
        }
        if notify && (nightly || guardOn) && Onboarding.isBundled {
            _ = try? await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound])
        }
        await store.loadSchedules()
    }

    private func finish(review: Bool, open: Bool = false) {
        Onboarding.markDone()
        savedStep = 0
        store.startGuard()
        if review || open || !showInMenuBar {
            nav.section = .overview
            if review { nav.review(plan: "stale") }
            openWindow(id: "main")
            NSApp.activate(ignoringOtherApps: true)
        }
        dismissWindow(id: "onboarding")
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
        .card()
        .draggable(url)
        .help("Drag into System Settings › Privacy & Security › Full Disk Access")
    }
}

/// Puts the CLI bundled in Poo.app on the PATH as ~/.local/bin/poo (a link, so Sparkle
/// updates it with the app). Never replaces a poo someone installed another way.
enum CommandLineTool {
    enum State: Equatable { case linked, separate(String), notInstalled, unavailable }

    static var bundled: URL? { Bundle.main.url(forResource: "poo", withExtension: nil, subdirectory: "bin") }
    static var link: URL { URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent(".local/bin/poo") }

    static var state: State {
        guard let bundled else { return .unavailable }
        let fm = FileManager.default
        if let dest = try? fm.destinationOfSymbolicLink(atPath: link.path) {
            return dest == bundled.path ? .linked : .notInstalled
        }
        if fm.isExecutableFile(atPath: link.path) { return .separate(link.path) }
        for p in ["/opt/homebrew/bin/poo", "/usr/local/bin/poo"] where fm.isExecutableFile(atPath: p) { return .separate(p) }
        return .notInstalled
    }

    /// Whether ~/.local/bin is on the PATH a new zsh would get (checked from ~/.zshrc and friends).
    static var onPath: Bool {
        ["/.zshrc", "/.zprofile", "/.profile", "/.bash_profile", "/.config/fish/config.fish"].contains { f in
            ((try? String(contentsOfFile: NSHomeDirectory() + f, encoding: .utf8)) ?? "").contains(".local/bin")
        }
    }

    /// Links the bundled CLI. Returns an error message, or nil on success.
    @discardableResult
    static func linkNow() -> String? {
        guard let bundled else { return "This build has no bundled CLI." }
        if case .separate = state { return nil }
        let fm = FileManager.default
        do {
            try fm.createDirectory(at: link.deletingLastPathComponent(), withIntermediateDirectories: true)
            if (try? fm.destinationOfSymbolicLink(atPath: link.path)) != nil { try fm.removeItem(at: link) }
            try fm.createSymbolicLink(at: link, withDestinationURL: bundled)
            return nil
        } catch {
            return error.localizedDescription
        }
    }

    static let pathLine = "echo 'export PATH=\"$HOME/.local/bin:$PATH\"' >> ~/.zshrc"

    /// The app menu's Install Command-Line Tool…
    static func install() {
        let alert = NSAlert()
        defer { NSApp.activate(); alert.runModal() }
        switch state {
        case .unavailable:
            alert.messageText = "This build has no bundled CLI"
            alert.informativeText = "Install it with: curl -fsSL poo.morajabi.com/install | sh"
            return
        case .separate(let p):
            alert.messageText = "poo is already installed"
            alert.informativeText = "\(p) is a separate install that updates itself. Poo left it as is."
            return
        case .linked, .notInstalled:
            break
        }
        if let err = linkNow() {
            alert.messageText = "Could not install poo"
            alert.informativeText = err
            return
        }
        alert.messageText = "poo is ready in Terminal"
        alert.informativeText = "\(link.path) runs the CLI inside Poo.app and updates with it."
            + (onPath ? "" : "\n\nAdd ~/.local/bin to your PATH:\n\(pathLine)")
    }
}
