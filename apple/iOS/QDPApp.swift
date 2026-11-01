import SwiftUI

/// QDP for iPhone and iPad: the computers to stream from, and the stream
/// full screen.
@main
struct QDPApp: App {
    @State private var store = HostStore()

    init() {
        Core.start()
    }

    var body: some Scene {
        WindowGroup {
            HostsScreen(store: store)
        }
    }
}

/// The computers to stream from, with Settings and adding a computer by its
/// address in the toolbar.
struct HostsScreen: View {
    var store: HostStore
    @State private var adding = false
    @State private var showingSettings = false
    @State private var streaming: StreamTarget?

    /// How long a sheet takes to go away, before the stream covers the
    /// screen.
    private static let sheetDismissal: TimeInterval = 0.45

    var body: some View {
        NavigationStack {
            HostListView(store: store, open: { streaming = $0 }, addByAddress: { adding = true })
                .navigationTitle("QDP")
                .toolbar {
                    ToolbarItem(placement: .topBarLeading) {
                        Button("Settings", systemImage: "gearshape") { showingSettings = true }
                    }
                    ToolbarItem(placement: .topBarTrailing) {
                        Button("Add Computer", systemImage: "plus") { adding = true }
                    }
                }
        }
        .sheet(isPresented: $adding) {
            AddHostView { target in
                // The stream covers the screen once the sheet has gone.
                DispatchQueue.main.asyncAfter(deadline: .now() + Self.sheetDismissal) {
                    streaming = target
                }
            }
        }
        .sheet(isPresented: $showingSettings) {
            NavigationStack {
                SettingsView()
                    .navigationTitle("Settings")
                    .navigationBarTitleDisplayMode(.inline)
                    .toolbar {
                        ToolbarItem(placement: .confirmationAction) {
                            Button("Done") { showingSettings = false }
                        }
                    }
            }
        }
        .fullScreenCover(item: $streaming) { target in
            StreamScreen(target: target, store: store)
        }
        .onAppear { store.windowAppeared() }
    }
}
