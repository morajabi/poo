import SwiftUI

/// What a scan is doing, in words a person cares about, with live numbers.
/// Used wherever there is no report yet (first launch, onboarding) and as a strip during
/// a full rescan.
struct ScanProgressView: View {
    @Environment(Store.self) private var store
    var compact = false

    /// The CLI's phases, in order.
    static let steps: [(phase: String, title: String)] = [
        ("discovering", "Finding your projects"),
        ("scanning", "Measuring build output and caches"),
        ("scanning worktrees", "Measuring worktrees"),
        ("checking worktrees", "Checking worktrees for uncommitted work"),
        ("classifying", "Sorting out what's safe to clean"),
    ]

    static func title(_ phase: String?) -> String {
        switch phase {
        case nil, "starting": return "Getting ready"
        case "updating": return "Catching up on changes"
        case let p?: return steps.first { $0.phase == p }?.title ?? p.capitalized
        }
    }

    static func fraction(_ phase: String?) -> Double {
        guard let p = phase, let i = steps.firstIndex(where: { $0.phase == p }) else { return 0.04 }
        return Double(i + 1) / Double(steps.count + 1)
    }

    var body: some View {
        let p = store.progress
        if compact {
            HStack(spacing: 10) {
                ProgressView().controlSize(.small)
                Text(Self.title(p?.phase)).font(.callout.weight(.medium)).lineLimit(1).fixedSize()
                if let p, p.bytes > 0 {
                    Text("\(Fmt.bytes(p.bytes)) · \(p.files.formatted()) files")
                        .font(.callout).foregroundStyle(.secondary).monospacedDigit()
                        .lineLimit(1).fixedSize()
                        .contentTransition(.numericText())
                }
                Spacer(minLength: 8)
                if let d = p?.detail, !d.isEmpty {
                    Text(Fmt.tilde(d)).font(.caption).foregroundStyle(.tertiary)
                        .lineLimit(1).truncationMode(.middle)
                        .frame(maxWidth: 260, alignment: .trailing)
                }
            }
            .animation(.default, value: p?.bytes)
        } else {
            VStack(spacing: 14) {
                Image(systemName: "internaldrive")
                    .font(.system(size: 40, weight: .light))
                    .foregroundStyle(.secondary)
                    .symbolEffect(.pulse, options: .repeating)
                VStack(spacing: 4) {
                    Text(Self.title(p?.phase))
                        .font(.title3.weight(.semibold))
                        .contentTransition(.opacity)
                    Text(p.map { "\(Fmt.bytes($0.bytes)) in \($0.files.formatted()) files" } ?? "Looking at your dev folders")
                        .foregroundStyle(.secondary)
                        .monospacedDigit()
                        .contentTransition(.numericText())
                }
                ProgressView(value: Self.fraction(p?.phase))
                    .frame(maxWidth: 280)
                Text(p.map { Fmt.tilde($0.detail) } ?? " ")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .frame(maxWidth: 360)
                if store.report == nil {
                    Text("The first look takes up to a minute. After that Poo keeps up as files change.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.center)
                        .frame(maxWidth: 320)
                }
            }
            .animation(.smooth, value: p?.phase)
            .animation(.default, value: p?.bytes)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .padding()
        }
    }
}

/// The main window with no report: scanning, failed, or never scanned.
struct NoReportView: View {
    @Environment(Store.self) private var store

    var body: some View {
        if store.scanning {
            ScanProgressView()
        } else {
            ContentUnavailableView {
                Label(store.error == nil ? "No scan yet" : "The scan didn't finish", systemImage: "internaldrive")
            } description: {
                Text(store.error ?? "Poo looks at your dev folders and tool caches. Nothing is cleaned without your review.")
            } actions: {
                Button("Scan Now") { Task { await store.scan() } }
                    .buttonStyle(.borderedProminent)
                    .controlSize(.large)
            }
        }
    }
}
