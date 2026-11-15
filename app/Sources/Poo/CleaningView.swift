import SwiftUI

/// The review sheet once a clean starts: a sweeping broom, live progress, then what happened.
struct CleaningView: View {
    @Bindable var session: CleanSession
    var onShowBackups: (() -> Void)? = nil
    var onDone: () -> Void

    var body: some View {
        Group {
            switch session.phase {
            case .running: running
            case .finished: finished
            case .failed(let message): failed(message)
            }
        }
        .padding(24)
        .frame(width: 620)
        .animation(.smooth(duration: 0.35), value: session.phase)
    }

    // MARK: Running

    private var running: some View {
        VStack(spacing: 16) {
            SweepAnimation()
                .frame(height: 110)
            VStack(spacing: 4) {
                Text(session.cancelling ? "Stopping after this item…" : "Cleaning \(min(session.done + 1, max(session.total, 1))) of \(session.total)")
                    .font(.headline)
                    .monospacedDigit()
                Text(session.current.flatMap { $0.isEmpty ? nil : $0 } ?? " ")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .contentTransition(.opacity)
            }
            VStack(spacing: 6) {
                if session.streaming {
                    ProgressView(value: session.fraction)
                        .animation(.easeOut(duration: 0.3), value: session.fraction)
                } else {
                    ProgressView().progressViewStyle(.linear)
                }
                HStack {
                    Text("Freed \(Fmt.bytes(session.freedBytes))")
                        .monospacedDigit()
                        .contentTransition(.numericText())
                        .animation(.default, value: session.freedBytes)
                    Spacer()
                    Text("of \(Fmt.bytes(session.totalBytes))").foregroundStyle(.secondary).monospacedDigit()
                }
                .font(.callout)
            }
            HStack {
                if session.dryRun {
                    Label("Dry run: nothing is deleted", systemImage: "eye").font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                Button("Stop") { session.cancel() }
                    .keyboardShortcut(.cancelAction)
                    .disabled(session.cancelling)
                    .help("Finish the item in progress, then stop")
            }
        }
    }

    // MARK: Finished

    @ViewBuilder
    private var finished: some View {
        let r = session.result
        let results = r?.results ?? []
        let skipped = results.filter { $0.status == "skipped" }
        let failed = results.filter { $0.status == "failed" }
        let notes = results.flatMap { $0.notes ?? [] }.reduce(into: [String]()) { if !$0.contains($1) { $0.append($1) } }
        VStack(spacing: 14) {
            ZStack {
                SuccessBurst()
                Image(systemName: (r?.cancelled == true) ? "stop.circle.fill" : "checkmark.circle.fill")
                    .font(.system(size: 44))
                    .foregroundStyle((r?.cancelled == true) ? Color.secondary : Color.green)
                    .symbolEffect(.bounce, value: session.phase)
            }
            .frame(height: 80)
            VStack(spacing: 4) {
                Text("\(session.dryRun ? "Would free" : "Freed") \(Fmt.bytes(r?.freed_bytes ?? session.freedBytes))")
                    .font(.title2.weight(.semibold))
                    .monospacedDigit()
                if let before = session.freeBefore, let after = r?.disk_free_after, !session.dryRun {
                    Text("\(Fmt.bytes(before)) → \(Fmt.bytes(after)) free").foregroundStyle(.secondary).monospacedDigit()
                }
                Text(summaryLine(r)).font(.callout).foregroundStyle(.secondary)
            }
            if !skipped.isEmpty || !failed.isEmpty || !notes.isEmpty || r?.snapshot_note != nil {
                ScrollView {
                    VStack(alignment: .leading, spacing: 12) {
                        if !notes.isEmpty {
                            group("Kept safe", "checkmark.shield", .green) {
                                ForEach(notes, id: \.self) { Text($0) }
                            }
                        }
                        if !skipped.isEmpty {
                            group("Skipped", "forward", .secondary) {
                                ForEach(skipped) { i in resultRow(i) }
                            }
                        }
                        if !failed.isEmpty {
                            group("Couldn't clean", "exclamationmark.triangle", .orange) {
                                ForEach(failed) { i in resultRow(i) }
                            }
                        }
                        if let note = r?.snapshot_note {
                            group("Space not back yet", "clock.arrow.circlepath", .secondary) {
                                Text(note)
                            }
                        }
                    }
                    .font(.callout)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(12)
                }
                .frame(maxHeight: 220)
                .fixedSize(horizontal: false, vertical: true)
                .card()
            }
            HStack {
                if !notes.isEmpty, let onShowBackups {
                    Button("Show Backups") { onShowBackups() }
                }
                Spacer()
                Button("Done", action: onDone)
                    .keyboardShortcut(.defaultAction)
            }
        }
    }

    private func summaryLine(_ r: CleanResult?) -> String {
        guard let r else { return "" }
        var parts = ["\(r.cleaned) cleaned"]
        if r.skipped > 0 { parts.append("\(r.skipped) skipped") }
        if r.failed > 0 { parts.append("\(r.failed) failed") }
        if r.cancelled == true { parts.append("stopped early") }
        if r.dry_run { parts.append("dry run, nothing deleted") }
        return parts.joined(separator: " · ")
    }

    private func group<C: View>(_ title: String, _ symbol: String, _ color: Color, @ViewBuilder _ content: () -> C) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Label(title, systemImage: symbol)
                .font(.callout.weight(.semibold))
                .foregroundStyle(color == .secondary ? Color.primary : color)
            VStack(alignment: .leading, spacing: 3) { content() }
                .padding(.leading, 24)
                .foregroundStyle(.secondary)
        }
    }

    private func resultRow(_ i: CleanItemResult) -> some View {
        HStack(alignment: .firstTextBaseline) {
            Text(i.name).foregroundStyle(.primary).lineLimit(1).truncationMode(.middle)
            Text(i.reason ?? "").lineLimit(2)
            Spacer(minLength: 0)
        }
    }

    // MARK: Failed

    private func failed(_ message: String) -> some View {
        VStack(spacing: 14) {
            Image(systemName: "exclamationmark.triangle.fill").font(.system(size: 36)).foregroundStyle(.orange)
            Text("The clean didn't finish").font(.headline)
            Text(message).font(.callout).foregroundStyle(.secondary).multilineTextAlignment(.center)
                .textSelection(.enabled)
            HStack { Spacer(); Button("Done", action: onDone).keyboardShortcut(.defaultAction) }
        }
    }
}

/// A broom sweeping dust into a bin. Still when Reduce Motion is on.
struct SweepAnimation: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        if reduceMotion {
            HStack(spacing: 40) {
                Text("🧹").font(.system(size: 48))
                Image(systemName: "trash").font(.system(size: 34, weight: .light)).foregroundStyle(.secondary)
            }
        } else {
            TimelineView(.animation) { tl in
                let t = tl.date.timeIntervalSinceReferenceDate
                GeometryReader { geo in
                    // Everything scales with the frame, so the same scene works full size and compact.
                    let w = geo.size.width, h = geo.size.height
                    let unit = min(w / 300, h / 110)
                    let floor = h - 12 * unit
                    let span = min(w, 300 * unit)
                    let left = (w - span) / 2
                    let binX = left + span * 0.82
                    let broomX = left + span * 0.26 + CGFloat(sin(t * 4.2)) * 16 * unit
                    ZStack {
                        Canvas { ctx, _ in
                            var line = Path()
                            line.move(to: CGPoint(x: left, y: floor))
                            line.addLine(to: CGPoint(x: left + span, y: floor))
                            ctx.stroke(line, with: .color(.secondary.opacity(0.25)), lineWidth: 1)
                            // Dust: each particle hops from the broom to the bin on its own phase.
                            for i in 0..<16 {
                                let seed = Double(i) * 0.6180339
                                let speed = 0.55 + seed.truncatingRemainder(dividingBy: 1) * 0.35
                                let p = (t * speed + seed).truncatingRemainder(dividingBy: 1)
                                let startX = broomX + 18 * unit
                                let x = startX + (binX - 14 * unit - startX) * p
                                let hop = sin(p * .pi) * (10 + Double(i % 4) * 7) * unit
                                let y = floor - 3 * unit - hop
                                let size = (2.0 + Double(i % 3)) * max(unit, 0.7)
                                let alpha = p < 0.85 ? 0.8 : max(0, (1 - p) / 0.15) * 0.8
                                let color: Color = i % 5 == 0 ? .accentColor : .secondary
                                ctx.fill(Path(ellipseIn: CGRect(x: x, y: y, width: size, height: size)), with: .color(color.opacity(alpha)))
                            }
                        }
                        Text("🧹")
                            .font(.system(size: 50 * unit))
                            .rotationEffect(.degrees(-28 + sin(t * 4.2) * 14), anchor: .topTrailing)
                            .position(x: broomX, y: floor - 30 * unit)
                        Image(systemName: "trash")
                            .font(.system(size: 30 * unit, weight: .light))
                            .foregroundStyle(.secondary)
                            .position(x: binX, y: floor - 17 * unit)
                    }
                }
            }
        }
    }
}

/// A short, quiet burst of dots behind the success check. Nothing when Reduce Motion is on.
struct SuccessBurst: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var start = Date()

    var body: some View {
        if !reduceMotion {
            TimelineView(.animation) { tl in
                let e = tl.date.timeIntervalSince(start)
                Canvas { ctx, size in
                    guard e < 1.4 else { return }
                    let p = e / 1.4
                    let c = CGPoint(x: size.width / 2, y: size.height / 2)
                    let colors: [Color] = [.green, .accentColor, .mint, .teal, .yellow]
                    for i in 0..<22 {
                        let angle = Double(i) / 22 * 2 * .pi + Double(i % 3) * 0.2
                        let dist = 26 + p * (50 + Double(i % 4) * 12)
                        let pt = CGPoint(x: c.x + cos(angle) * dist, y: c.y + sin(angle) * dist + p * p * 18)
                        let r = 2.6 * (1 - p * 0.6)
                        ctx.fill(Path(ellipseIn: CGRect(x: pt.x - r, y: pt.y - r, width: r * 2, height: r * 2)),
                                 with: .color(colors[i % colors.count].opacity(1 - p)))
                    }
                }
            }
            .allowsHitTesting(false)
            .onAppear { start = Date() }
        }
    }
}

/// Debug window for the cleaning UI. Always a dry run or a simulation; never deletes.
struct CleaningPreview: View {
    @Environment(Store.self) private var store
    @State private var session: CleanSession?

    var body: some View {
        Group {
            if let session {
                CleaningView(session: session) { NSApp.keyWindow?.close() }
            } else {
                ProgressView().frame(width: 520, height: 300)
            }
        }
        .task {
            guard session == nil, let plan = await store.plan("stale") else { return }
            let free = store.report?.disk.free ?? 0
            if CommandLine.arguments.contains("--preview-cleaning=fake") {
                session = CleanSession.fake(items: plan.items, freeBefore: free, speed: 0.5)
            } else {
                session = store.startClean(plan.items, mode: .safe, dryRun: true)
            }
        }
    }
}
