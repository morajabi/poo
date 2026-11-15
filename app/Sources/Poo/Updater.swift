import AppKit
import Foundation
import Observation
import Sparkle

/// App updates through Sparkle 2. Releases publish an EdDSA-signed `appcast.xml`; the
/// bundled CLI lives inside Poo.app, so updating the app updates the CLI with it.
///
/// Sparkle only starts in a bundled build whose Info.plist carries `SUFeedURL` and
/// `SUPublicEDKey` (release builds). `swift run` and unsigned local builds leave it off.
@MainActor
@Observable
final class Updater: NSObject {
    /// The version Sparkle found, while one is waiting to be installed.
    var available: String?
    var canCheck = false
    var status: String?

    var automaticallyChecks: Bool {
        get { access(keyPath: \.automaticallyChecks); return controller.updater.automaticallyChecksForUpdates }
        set { withMutation(keyPath: \.automaticallyChecks) { controller.updater.automaticallyChecksForUpdates = newValue } }
    }

    var automaticallyDownloads: Bool {
        get { access(keyPath: \.automaticallyDownloads); return controller.updater.automaticallyDownloadsUpdates }
        set { withMutation(keyPath: \.automaticallyDownloads) { controller.updater.automaticallyDownloadsUpdates = newValue } }
    }

    /// False in builds without a feed or signing key; the settings say why.
    let enabled: Bool

    @ObservationIgnored private var controller: SPUStandardUpdaterController!
    @ObservationIgnored private var observation: NSKeyValueObservation?

    var currentVersion: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "dev"
    }

    override init() {
        let info = Bundle.main.infoDictionary ?? [:]
        let key = (info["SUPublicEDKey"] as? String) ?? ""
        let feed = (info["SUFeedURL"] as? String) ?? ""
        enabled = Bundle.main.bundleURL.pathExtension == "app" && !key.isEmpty && !feed.isEmpty
            && ProcessInfo.processInfo.environment["POO_NO_SPARKLE"] == nil
        super.init()
        controller = SPUStandardUpdaterController(startingUpdater: false, updaterDelegate: self, userDriverDelegate: self)
        guard enabled else {
            status = "Updates are off in this build (no Sparkle feed or key)."
            return
        }
        controller.startUpdater()
        observation = controller.updater.observe(\.canCheckForUpdates, options: [.initial, .new]) { [weak self] u, _ in
            let can = u.canCheckForUpdates
            Task { @MainActor in self?.canCheck = can }
        }
    }

    /// Shows Sparkle's update window: checks, then offers to download and install.
    func checkForUpdates() {
        guard enabled else { return }
        NSApp.activate(ignoringOtherApps: true)
        controller.checkForUpdates(nil)
    }

    /// Kept for the menu bar's Update button.
    func install() async { checkForUpdates() }
}

extension Updater: SPUUpdaterDelegate {
    nonisolated func updater(_ updater: SPUUpdater, didFindValidUpdate item: SUAppcastItem) {
        let v = item.displayVersionString
        Task { @MainActor in self.available = v }
    }

    nonisolated func updaterDidNotFindUpdate(_ updater: SPUUpdater) {
        Task { @MainActor in self.available = nil }
    }

    nonisolated func updater(_ updater: SPUUpdater, didAbortWithError error: Error) {
        let msg = error.localizedDescription
        Task { @MainActor in self.status = msg }
    }
}

extension Updater: SPUStandardUserDriverDelegate {
    /// Poo is a menu bar app, so scheduled updates show as a gentle reminder
    /// (the menu's Update button) instead of a window popping up over other work.
    nonisolated var supportsGentleScheduledUpdateReminders: Bool { true }

    nonisolated func standardUserDriverShouldHandleShowingScheduledUpdate(_ update: SUAppcastItem, andInImmediateFocus immediateFocus: Bool) -> Bool {
        immediateFocus
    }

    nonisolated func standardUserDriverWillHandleShowingUpdate(_ handleShowingUpdate: Bool, forUpdate update: SUAppcastItem, state: SPUUserUpdateState) {
        let v = update.displayVersionString
        Task { @MainActor in
            self.available = v
            if handleShowingUpdate { NSApp.activate(ignoringOtherApps: true) }
        }
    }

    nonisolated func standardUserDriverWillFinishUpdateSession() {
        Task { @MainActor in self.available = nil }
    }
}
