import SwiftUI

/// A stream in a window of its own: the host's picture, what the stream is
/// doing until it shows, the pairing code when the host asks for one, and
/// the statistics if shown.
struct StreamWindowView: View {
    let store: HostStore
    @State private var model: StreamModel
    @AppStorage(SettingsKey.showStats) private var showStats = false
    @Environment(\.dismiss) private var dismiss

    init(target: StreamTarget, store: HostStore) {
        self.store = store
        _model = State(initialValue: StreamModel(target: target, store: store))
    }

    var body: some View {
        ZStack(alignment: .topLeading) {
            Color.black
            MacStreamView(model: model)
            status
            if showStats, let stats = model.stats {
                Text(stats.summary)
                    .font(.caption.monospacedDigit())
                    .padding(.horizontal, 10)
                    .padding(.vertical, 6)
                    .glassEffect(.regular, in: .capsule)
                    .padding(12)
            }
        }
        .frame(minWidth: 480, minHeight: 270)
        .navigationTitle(model.target.name)
        .sheet(isPresented: pairing) {
            PairingView(hostName: model.target.name, retry: retry) { code in
                model.answerPairing(code)
            }
        }
        .onAppear { store.streamStarted() }
        .onDisappear {
            model.stop()
            store.streamEnded()
        }
        .onChange(of: model.phase) { _, phase in
            if phase == .ended { dismiss() }
        }
    }

    /// What the stream is doing until its picture shows, or why it ended.
    @ViewBuilder private var status: some View {
        switch model.phase {
        case .connecting, .starting, .reconnecting:
            VStack(spacing: 12) {
                ProgressView()
                    .controlSize(.large)
                Text(statusText)
                    .foregroundStyle(.white)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        case let .failed(error):
            ContentUnavailableView {
                Label("Couldn't Stream from \(model.target.name)", systemImage: "exclamationmark.triangle")
            } description: {
                Text(error)
            } actions: {
                Button("Close") { dismiss() }
                    .keyboardShortcut(.defaultAction)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(.black)
        case .pairing, .streaming, .ended:
            EmptyView()
        }
    }

    private var statusText: String {
        switch model.phase {
        case .connecting: "Connecting to \(model.target.name)…"
        case .reconnecting: "Connection lost. Reconnecting…"
        default: "Starting the stream…"
        }
    }

    private var pairing: Binding<Bool> {
        Binding(
            get: { if case .pairing = model.phase { true } else { false } },
            set: { _ in }
        )
    }

    private var retry: Bool {
        if case let .pairing(retry) = model.phase { retry } else { false }
    }
}

/// The view the stream draws into, with the user's input.
struct MacStreamView: NSViewRepresentable {
    let model: StreamModel

    func makeNSView(context: Context) -> StreamNSView {
        StreamNSView(model: model)
    }

    func updateNSView(_ view: StreamNSView, context: Context) {}

    static func dismantleNSView(_ view: StreamNSView, coordinator: ()) {
        view.tearDown()
    }
}
