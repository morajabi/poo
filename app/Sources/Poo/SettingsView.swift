import AppKit
import SwiftUI

struct SettingsView: View {
    @Environment(Store.self) private var store

    var body: some View {
        TabView {
            GeneralSettings().tabItem { Label("General", systemImage: "gearshape") }
            BucketSettings().tabItem { Label("Buckets", systemImage: "square.stack.3d.up") }
            RuleSettings().tabItem { Label("Rules", systemImage: "list.bullet.rectangle") }
            AgentSettings().tabItem { Label("Agents", systemImage: "sparkles") }
            ModSettings().tabItem { Label("Mods", systemImage: "puzzlepiece.extension") }
        }
        .task { await store.loadSettings() }
    }
}

struct GeneralSettings: View {
    @Environment(Store.self) private var store
    @Environment(Updater.self) private var updater

    var body: some View {
        Form {
            Section("Dev folders") {
                if let roots = store.roots {
                    if roots.detected {
                        Text("Detected automatically. Add or remove one to pin the list.").font(.caption).foregroundStyle(.secondary)
                    }
                    ForEach(roots.roots, id: \.self) { r in
                        HStack {
                            Image(systemName: "folder")
                            Text(Fmt.tilde(r))
                            Spacer()
                            Button("Remove") { Task { await store.configure(["roots", "rm", r], rescan: true) } }
                                .buttonStyle(.borderless)
                        }
                    }
                }
                Button("Add folder…") { pickFolder() }
            }
            Section("Updates") {
                HStack {
                    Text("Poo \(Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "dev")")
                    Spacer()
                    if let v = updater.available {
                        Button("Install \(v)") { Task { await updater.install() } }
                    } else {
                        Button(updater.checking ? "Checking…" : "Check for updates") { Task { await updater.check(userInitiated: true) } }
                            .disabled(updater.checking)
                    }
                }
                if let s = updater.status { Text(s).font(.caption).foregroundStyle(.secondary) }
            }
            Section("Data") {
                Button("Rescan now") { Task { await store.scan() } }
                Button("Open config folder") {
                    NSWorkspace.shared.open(URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent(".config/poo"))
                }
            }
        }
        .formStyle(.grouped)
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
                await store.scan()
            }
        }
    }
}

struct BucketSettings: View {
    @Environment(Store.self) private var store
    @State private var newID = ""
    @State private var newName = ""

    var body: some View {
        Form {
            Section {
                ForEach(store.catalog?.buckets ?? []) { b in BucketRow(bucket: b) }
            } header: {
                Text("Buckets group items in the menu bar, the window, the CLI and MCP. Changes are saved to config.toml.")
                    .font(.caption)
            }
            Section("New bucket") {
                HStack {
                    TextField("id", text: $newID).frame(width: 120)
                    TextField("Name", text: $newName)
                    Button("Add") {
                        let id = newID.lowercased().replacingOccurrences(of: " ", with: "-")
                        Task { await store.configure(["buckets", "add", id, "--name", newName.isEmpty ? newID : newName]) }
                        newID = ""; newName = ""
                    }
                    .disabled(newID.isEmpty)
                }
                Text("Move rules into it from the Rules tab.").font(.caption).foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
    }
}

struct BucketRow: View {
    @Environment(Store.self) private var store
    var bucket: CatalogBucket
    @State private var name = ""
    @State private var color = Color.gray

    var body: some View {
        HStack {
            Image(systemName: bucket.symbol).foregroundStyle(Color(hex: bucket.color)).frame(width: 20)
            TextField("Name", text: $name)
                .onSubmit { Task { await store.configure(["buckets", "set", bucket.id, "--name", name], rescan: true) } }
            ColorPicker("", selection: $color, supportsOpacity: false)
                .labelsHidden()
                .onChange(of: color) { _, c in
                    guard let hex = c.hexString, hex.lowercased() != bucket.color.lowercased() else { return }
                    Task { await store.configure(["buckets", "set", bucket.id, "--color", hex], rescan: true) }
                }
            Toggle("Hidden", isOn: Binding(get: { bucket.hidden }, set: { h in
                Task { await store.configure(["buckets", "set", bucket.id, "--hidden", h ? "true" : "false"], rescan: true) }
            }))
            .toggleStyle(.checkbox)
            Text(bucket.id).font(.caption.monospaced()).foregroundStyle(.secondary).frame(width: 90, alignment: .trailing)
        }
        .onAppear {
            name = bucket.name
            color = Color(hex: bucket.color)
        }
    }
}

struct RuleSettings: View {
    @Environment(Store.self) private var store
    @State private var query = ""

    var body: some View {
        let rules = (store.catalog?.rules ?? []).filter {
            query.isEmpty || $0.id.localizedCaseInsensitiveContains(query) || $0.name.localizedCaseInsensitiveContains(query) || $0.bucket.localizedCaseInsensitiveContains(query)
        }
        VStack(alignment: .leading) {
            TextField("Search \(store.catalog?.rules.count ?? 0) rules", text: $query).textFieldStyle(.roundedBorder)
            List(rules) { r in
                HStack(alignment: .top) {
                    Toggle("", isOn: Binding(get: { r.enabled }, set: { on in
                        Task { await store.configure(["rules", on ? "enable" : "disable", r.id], rescan: true) }
                    }))
                    .labelsHidden()
                    VStack(alignment: .leading, spacing: 2) {
                        HStack {
                            Text(r.name).font(.callout.weight(.medium))
                            Text(r.id).font(.caption.monospaced()).foregroundStyle(.secondary)
                        }
                        Text(r.description).font(.caption).foregroundStyle(.secondary).lineLimit(2)
                    }
                    Spacer()
                    Picker("", selection: Binding(get: { r.bucket }, set: { b in
                        Task { await store.configure(["rules", "set", r.id, "bucket=\"\(b)\""], rescan: true) }
                    })) {
                        ForEach(store.catalog?.buckets ?? []) { b in Text(b.name).tag(b.id) }
                    }
                    .labelsHidden()
                    .frame(width: 150)
                    if r.cost != "none" && r.tier == nil {
                        Stepper(value: Binding(get: { r.warm_days }, set: { d in
                            Task { await store.configure(["rules", "set", r.id, "warm_days=\(d)"], rescan: true) }
                        }), in: 0...365) {
                            Text(r.warm_days == 0 ? "default" : "\(r.warm_days)d").font(.caption).monospacedDigit().frame(width: 50)
                        }
                        .help("Used within this many days counts as warm (in use)")
                    }
                }
            }
        }
        .padding()
    }
}

struct AgentSettings: View {
    @Environment(Store.self) private var store
    @State private var copied = false

    static let setupPrompt = "Set up poo for me: run `poo setup --agent`, read the brief, combine it with what you know about my projects and workflows, ask me only what you can't infer, then apply the configuration."

    var body: some View {
        Form {
            Section("Coding agents") {
                ForEach(store.integrations.filter(\.detected)) { t in
                    HStack {
                        Text(t.name)
                        Spacer()
                        Label(t.skill_current ? "Skill" : "No skill", systemImage: t.skill_current ? "checkmark.circle.fill" : "circle")
                            .foregroundStyle(t.skill_current ? .green : .secondary)
                        Label(t.mcp_registered ? "MCP" : "No MCP", systemImage: t.mcp_registered ? "checkmark.circle.fill" : "circle")
                            .foregroundStyle(t.mcp_registered ? .green : .secondary)
                        Button(t.skill_current && t.mcp_registered ? "Reinstall" : "Install") {
                            Task { await store.configure(["integration", "install", t.id]) }
                        }
                    }
                    .font(.callout)
                }
                Button("Install for all detected agents") { Task { await store.configure(["integration", "install", "all"]) } }
            }
            Section("Let your agent configure Poo") {
                Text(Self.setupPrompt).font(.callout).textSelection(.enabled)
                Button(copied ? "Copied" : "Copy prompt") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(Self.setupPrompt, forType: .string)
                    copied = true
                }
            }
        }
        .formStyle(.grouped)
    }
}

struct ModSettings: View {
    @Environment(Store.self) private var store

    var body: some View {
        Form {
            Section("Mods") {
                ForEach(store.catalog?.mods ?? []) { m in
                    HStack {
                        VStack(alignment: .leading) {
                            Text(m.name)
                            Text(m.builtin ? "built in · \(m.rules) rules" : "\(m.path.map(Fmt.tilde) ?? "") · \(m.rules) rules")
                                .font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        Toggle("", isOn: Binding(get: { m.enabled }, set: { on in
                            Task { await store.configure(["mods", on ? "enable" : "disable", m.id], rescan: true) }
                        }))
                        .labelsHidden()
                    }
                }
                if let errors = store.catalog?.errors, !errors.isEmpty {
                    ForEach(errors, id: \.self) { Text($0).font(.caption).foregroundStyle(.red) }
                }
            }
            Section {
                Button("Open mods folder") {
                    let url = URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent(".config/poo/mods")
                    try? FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
                    NSWorkspace.shared.open(url)
                }
                Text("Mods teach Poo new folders, tweak or disable rules, add buckets, plans and guards. Ask your coding agent to write one: it can run `poo mods docs`.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
    }
}

extension Color {
    var hexString: String? {
        guard let c = NSColor(self).usingColorSpace(.sRGB) else { return nil }
        return String(format: "#%02X%02X%02X", Int(c.redComponent * 255), Int(c.greenComponent * 255), Int(c.blueComponent * 255))
    }
}
