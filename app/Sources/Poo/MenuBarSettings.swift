import SwiftUI

/// Settings > Menu Bar: what the status item shows and which sections the panel lists.
/// Everything is stored in config.toml `[menubar]`, so `poo config set menubar.…` works too.
struct MenuBarSettingsView: View {
    @Environment(Store.self) private var store
    @AppStorage("showInMenuBar") private var showInMenuBar = true

    private static let thresholds = ["5%", "10%", "15%", "20%", "10G", "20G", "50G"]

    var body: some View {
        let cfg = store.menubar
        Form {
            Section {
                Toggle("Show Poo in the menu bar", isOn: $showInMenuBar)
                    .onChange(of: showInMenuBar) { _, _ in AppDelegate.updateActivationPolicy(windowOpen: true) }
                LabeledContent("Preview") {
                    MenuBarLabel(store: store, preview: true)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 3)
                        .background(.quaternary, in: RoundedRectangle(cornerRadius: 5))
                }
            }
            Section("Menu bar item") {
                ForEach(MenubarSettings.Part.allCases) { p in
                    Toggle(p.label, isOn: Binding(
                        get: { cfg.shows(p) },
                        set: { on in
                            var parts = Set(cfg.parts)
                            if on { parts.insert(p) } else { parts.remove(p) }
                            let title = MenubarSettings.title(parts)
                            Task { await store.setMenubar("title", title) { $0.title = title } }
                        }
                    ))
                }
                Toggle("Turn orange when space is low", isOn: Binding(
                    get: { cfg.warn },
                    set: { on in Task { await store.setMenubar("warn", on ? "true" : "false") { $0.warn = on } } }
                ))
                Picker("Low means under", selection: Binding(
                    get: { cfg.warn_below },
                    set: { v in Task { await store.setMenubar("warn_below", "\"\(v)\"") { $0.warn_below = v } } }
                )) {
                    ForEach(Set(Self.thresholds + [cfg.warn_below]).sorted(by: Self.thresholdOrder), id: \.self) { t in
                        Text(t.hasSuffix("%") ? "\(t) free" : "\(t)B free").tag(t)
                    }
                }
                .disabled(!cfg.warn)
                Toggle("Notify me when space runs low", isOn: Binding(
                    get: { cfg.notify },
                    set: { on in Task { await store.setMenubar("notify", on ? "true" : "false") { $0.notify = on } } }
                ))
            }
            Section {
                List {
                    ForEach(orderedSections(cfg)) { s in
                        Toggle(s.label, isOn: Binding(
                            get: { cfg.shows(s) },
                            set: { on in setSections(cfg, s, on) }
                        ))
                    }
                    .onMove { from, to in move(cfg, from, to) }
                }
                Picker("Order areas by", selection: Binding(
                    get: { cfg.area_order },
                    set: { v in Task { await store.setMenubar("area_order", "\"\(v)\"") { $0.area_order = v } } }
                )) {
                    Text("Size").tag("size")
                    Text("Bucket").tag("bucket")
                    Text("Name").tag("name")
                }
                Stepper("Show up to \(cfg.max_areas) areas", value: Binding(
                    get: { cfg.max_areas },
                    set: { v in Task { await store.setMenubar("max_areas", "\(v)") { $0.max_areas = v } } }
                ), in: 2...12)
            } header: {
                Text("Panel")
            } footer: {
                Text("Drag to reorder. Stored under [menubar] in config.toml.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
        .task { await store.loadMenubar() }
    }

    /// Shown sections in their order, then the hidden ones.
    private func orderedSections(_ cfg: MenubarSettings) -> [MenubarSettings.Section] {
        let on = cfg.sections.compactMap(MenubarSettings.Section.init(rawValue:))
        return on + MenubarSettings.Section.allCases.filter { !on.contains($0) }
    }

    private func save(_ list: [MenubarSettings.Section], shown: Set<MenubarSettings.Section>) {
        let ids = list.filter(shown.contains).map(\.rawValue)
        let value = "[" + ids.map { "\"\($0)\"" }.joined(separator: ", ") + "]"
        Task { await store.setMenubar("sections", value) { $0.sections = ids } }
    }

    private func setSections(_ cfg: MenubarSettings, _ s: MenubarSettings.Section, _ on: Bool) {
        var shown = Set(cfg.sections.compactMap(MenubarSettings.Section.init(rawValue:)))
        if on { shown.insert(s) } else { shown.remove(s) }
        save(orderedSections(cfg), shown: shown)
    }

    private func move(_ cfg: MenubarSettings, _ from: IndexSet, _ to: Int) {
        var list = orderedSections(cfg)
        list.move(fromOffsets: from, toOffset: to)
        save(list, shown: Set(cfg.sections.compactMap(MenubarSettings.Section.init(rawValue:))))
    }

    private static func thresholdOrder(_ a: String, _ b: String) -> Bool {
        let pa = a.hasSuffix("%"), pb = b.hasSuffix("%")
        if pa != pb { return pa }
        return (Double(a.filter { $0.isNumber || $0 == "." }) ?? 0) < (Double(b.filter { $0.isNumber || $0 == "." }) ?? 0)
    }
}
