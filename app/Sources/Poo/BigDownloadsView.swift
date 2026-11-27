import SwiftUI

/// Big Downloads: simulator runtimes, SDKs, toolchains and models. Each takes a long
/// download to get back, so Poo never removes them on its own, not even when space runs
/// out. The user opens each group, reads what an item is, and picks what can go.
struct BigDownloadsView: View {
    @Environment(Store.self) private var store
    @Environment(Navigation.self) private var nav
    @State private var picked: Set<String> = []
    @State private var expanded: Set<String> = []
    @State private var details: Set<String> = []

    struct Group: Identifiable {
        var name: String
        var items: [Item]
        var id: String { name }
        var bytes: UInt64 { items.reduce(0) { $0 + $1.total_bytes } }
    }

    private var groups: [Group] {
        Dictionary(grouping: store.bigDownloads, by: \.group)
            .map { Group(name: $0.key, items: $0.value) }
            .sorted { $0.bytes > $1.bytes }
    }

    private var selected: [Item] { store.bigDownloads.filter { picked.contains($0.id) } }

    var body: some View {
        let groups = groups
        List {
            Section {
                Text("These take a long download to get back, so Poo never removes them on its own, not even when space runs low. Open a group, check what each one is, and pick what you no longer need.")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            ForEach(groups) { g in
                Section {
                    DisclosureGroup(isExpanded: binding(g.id, in: $expanded)) {
                        ForEach(g.items) { i in row(i) }
                    } label: {
                        groupLabel(g)
                    }
                }
            }
        }
        .listStyle(.inset)
        .overlay {
            if store.report != nil, groups.isEmpty {
                ContentUnavailableView("No big downloads", systemImage: "arrow.down.circle",
                                       description: Text("Simulator runtimes, SDKs and models show up here when Poo finds them."))
            }
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            if !selected.isEmpty { bar }
        }
        .navigationTitle("Big Downloads")
        .task {
            if store.catalog == nil { await store.loadSettings() }
            if expanded.isEmpty, let first = groups.first { expanded = [first.id] }
        }
        .onChange(of: store.bigDownloads) { _, now in
            picked.formIntersection(now.map(\.id))
        }
    }

    // MARK: Rows

    private func groupLabel(_ g: Group) -> some View {
        let ids = Set(g.items.map(\.id))
        let on = ids.isSubset(of: picked)
        let some = !on && !ids.isDisjoint(with: picked)
        return HStack(spacing: 8) {
            Button {
                if on { picked.subtract(ids) } else { picked.formUnion(ids) }
            } label: {
                Image(systemName: on ? "checkmark.square.fill" : some ? "minus.square.fill" : "square")
                    .foregroundStyle(on || some ? Color.accentColor : .secondary)
            }
            .buttonStyle(.plain)
            .accessibilityLabel(on ? "Deselect \(g.name)" : "Select all \(g.name)")
            Text(g.name).font(.headline)
            Text("\(g.items.count)").font(.caption).foregroundStyle(.secondary).monospacedDigit()
            Spacer()
            Text(Fmt.bytes(g.bytes)).monospacedDigit().foregroundStyle(.secondary)
        }
    }

    private func row(_ i: Item) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Toggle("", isOn: Binding(get: { picked.contains(i.id) }, set: { on in
                    if on { picked.insert(i.id) } else { picked.remove(i.id) }
                }))
                .toggleStyle(.checkbox)
                .labelsHidden()
                VStack(alignment: .leading, spacing: 1) {
                    Text(i.name).lineLimit(1).truncationMode(.middle)
                    Text(summary(i)).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
                Spacer(minLength: 8)
                Text(Fmt.bytes(i.total_bytes)).monospacedDigit()
                Button {
                    withAnimation(.smooth) { toggle(i.id, in: &details) }
                } label: {
                    Image(systemName: "info.circle")
                        .symbolVariant(details.contains(i.id) ? .fill : .none)
                }
                .buttonStyle(.borderless)
                .help("Details")
            }
            if details.contains(i.id) { detail(i).padding(.leading, 26) }
        }
        .padding(.vertical, 2)
        .contextMenu {
            if !i.isVirtual { Button("Reveal in Finder") { store.reveal(i) } }
            Button("Copy Path") { Pasteboard.copy(i.path) }
        }
    }

    private func summary(_ i: Item) -> String {
        let used = i.lastUsedDate.map { "used \(Fmt.ago($0))" } ?? "never used"
        return ([used] + i.reasons.prefix(1)).joined(separator: " · ")
    }

    private func detail(_ i: Item) -> some View {
        let rule = store.catalog?.rules.first { $0.id == i.rule }
        return Grid(alignment: .leading, horizontalSpacing: 10, verticalSpacing: 4) {
            if let d = rule?.description, !d.isEmpty { line("What it is", d) }
            if let r = rule?.recommend { line("Tip", r) }
            line("Get it back", i.rebuild ?? "Download it again from the tool that installed it")
            line("Last used", Fmt.ago(i.lastUsedDate))
            if i.reasons.count > 1 { line("Notes", i.reasons.joined(separator: "; ")) }
            if !i.isVirtual { line("Where", Fmt.tilde(i.path)) }
        }
        .font(.caption)
        .textSelection(.enabled)
    }

    private func line(_ k: String, _ v: String) -> some View {
        GridRow {
            Text(k).foregroundStyle(.secondary).gridColumnAlignment(.trailing)
            Text(v).fixedSize(horizontal: false, vertical: true)
        }
    }

    private var bar: some View {
        let items = selected
        return HStack(spacing: 12) {
            Text("\(items.count) selected").foregroundStyle(.secondary).monospacedDigit()
            Button("Deselect All") { picked.removeAll() }.buttonStyle(.link)
            Spacer()
            Button("Remove \(Fmt.bytes(items.reduce(0) { $0 + $1.total_bytes }))…") {
                nav.review(items: items, mode: CleanMode.needed(for: items))
            }
            .buttonStyle(.borderedProminent)
            .disabled(store.busy)
            .help("Shows exactly what goes before removing anything")
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .floatingBar()
    }

    // MARK: Helpers

    private func binding(_ id: String, in set: Binding<Set<String>>) -> Binding<Bool> {
        Binding(get: { set.wrappedValue.contains(id) }, set: { on in
            withAnimation(.smooth) {
                if on { set.wrappedValue.insert(id) } else { set.wrappedValue.remove(id) }
            }
        })
    }

    private func toggle(_ id: String, in set: inout Set<String>) {
        if set.contains(id) { set.remove(id) } else { set.insert(id) }
    }
}
