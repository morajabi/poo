import SwiftUI

// Current macOS look with a macOS 15 floor: Liquid Glass and scroll-edge bars on macOS 26+,
// the closest system material before that.

extension View {
    /// A bar pinned above scrolling content. On macOS 26 content scrolls under it with the
    /// system edge effect instead of an opaque bar and a divider.
    @ViewBuilder
    func topBar<Bar: View>(@ViewBuilder _ bar: () -> Bar) -> some View {
        if #available(macOS 26, *) {
            safeAreaBar(edge: .top, spacing: 0, content: bar)
        } else {
            safeAreaInset(edge: .top, spacing: 0) {
                bar().background(.bar).overlay(alignment: .bottom) { Divider() }
            }
        }
    }

    /// A floating control bar at the bottom: a glass capsule on macOS 26, a bar before.
    @ViewBuilder
    func floatingBar() -> some View {
        if #available(macOS 26, *) {
            self.padding(.horizontal, 6)
                .glassEffect(.regular, in: .capsule)
                .padding(.horizontal, 16)
                .padding(.bottom, 12)
        } else {
            self.background(.bar).overlay(alignment: .top) { Divider() }
        }
    }

    /// Glass behind small transient chrome (toasts, hover cards).
    @ViewBuilder
    func glassBackground<S: Shape>(in shape: S) -> some View {
        if #available(macOS 26, *) {
            glassEffect(.regular, in: shape)
        } else {
            background(.regularMaterial, in: shape)
                .overlay(shape.stroke(.separator, lineWidth: 0.5))
        }
    }

    /// Grouped content on the window background, like a System Settings group.
    func card() -> some View {
        background(.background.secondary, in: .rect(cornerRadius: 12, style: .continuous))
    }
}

/// "95 items · Updated 2m ago" in the window subtitle, kept current while it's shown.
struct FreshSubtitle: ViewModifier {
    @Environment(Store.self) private var store
    var base: String
    @State private var now = Date()

    func body(content: Content) -> some View {
        content
            .navigationSubtitle(text)
            .task {
                while !Task.isCancelled {
                    try? await Task.sleep(for: .seconds(20))
                    now = Date()
                }
            }
    }

    private var text: String {
        var parts = base.isEmpty ? [] : [base]
        if store.refreshing {
            parts.append("Updating…")
        } else if let r = store.report {
            let s = max(0, Int(now.timeIntervalSince(r.scannedAt)))
            parts.append(s < 60 ? "Updated just now" : "Updated \(FreshnessLabel.age(r.scannedAt, now: now))")
        }
        return parts.joined(separator: " · ")
    }
}

extension View {
    func freshSubtitle(_ base: String) -> some View { modifier(FreshSubtitle(base: base)) }
}

/// Refresh in the toolbar: the arrow turns while a scan or a watched change is picked up.
struct RefreshToolbarButton: View {
    @Environment(Store.self) private var store

    var body: some View {
        Button { Task { await store.scan() } } label: {
            Label("Refresh", systemImage: "arrow.clockwise")
                .symbolEffect(.rotate, options: .repeat(.continuous), isActive: store.refreshing)
        }
        .disabled(store.scanning)
        .help("Refresh what changed (⌘R). Rescan Everything is in the View menu (⇧⌘R).")
    }
}
