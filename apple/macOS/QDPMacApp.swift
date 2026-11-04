import SwiftUI

/// QDP for the Mac: a window of the computers to stream from, a window for
/// each stream, and Settings.
@main
struct QDPMacApp: App {
    @State private var store = HostStore()

    init() {
        Core.start()
    }

    var body: some Scene {
        Window("Quick Desktop", id: WindowID.hosts) {
            HostBrowserView(store: store)
        }
        .defaultSize(width: 520, height: 560)

        WindowGroup("Stream", id: WindowID.stream, for: StreamTarget.self) { $target in
            if let target {
                StreamWindowView(target: target, store: store)
            }
        }
        .defaultSize(width: 1280, height: 720)
        .windowToolbarStyle(.unifiedCompact)
        .commands {
            CommandGroup(after: .toolbar) {
                StatisticsToggle()
            }
        }

        Settings {
            SettingsView()
        }
    }
}

/// The app's window identifiers.
enum WindowID {
    static let hosts = "hosts"
    static let stream = "stream"
}

/// The View menu's item that shows the stream's statistics.
private struct StatisticsToggle: View {
    @AppStorage(SettingsKey.showStats) private var showStats = false

    var body: some View {
        Toggle("Show Statistics", isOn: $showStats)
            .keyboardShortcut("i", modifiers: [.command, .option])
    }
}

/// The computers to stream from, with a toolbar button to add one by its
/// address.
struct HostBrowserView: View {
    var store: HostStore
    @Environment(\.openWindow) private var openWindow
    @State private var adding = false

    var body: some View {
        NavigationStack {
            HostListView(
                store: store,
                open: { openWindow(id: WindowID.stream, value: $0) },
                addByAddress: { adding = true }
            )
            .navigationTitle("Quick Desktop")
            .toolbar {
                ToolbarItem(placement: .primaryAction) {
                    Button("Add Computer", systemImage: "plus") { adding = true }
                        .help("Add a computer by its address")
                }
            }
        }
        .frame(minWidth: 420, minHeight: 360)
        .sheet(isPresented: $adding) {
            AddHostView(keepGatewayHost: store.keepGatewayHost, open: { openWindow(id: WindowID.stream, value: $0) })
        }
        .onAppear { store.windowAppeared() }
        .onDisappear { store.windowDisappeared() }
    }
}
