import AVFoundation
import SwiftUI

/// A stream full screen: the host's picture, an unobtrusive button that
/// opens the stream's menu, what the stream is doing until its picture
/// shows, the pairing code when the host asks for one, and the statistics
/// if shown.
struct StreamScreen: View {
    let store: HostStore
    @State private var model: StreamModel
    @State private var menuOpen = false
    @State private var keyboardRequests = 0
    /// Whether the on-screen gamepad shows: off as each stream starts.
    @State private var controllerShown = false
    @AppStorage(SettingsKey.showStats) private var showStats = false
    @AppStorage(SettingsKey.touchMode) private var touchMode = TouchMode.trackpad
    @Environment(\.dismiss) private var dismiss

    init(target: StreamTarget, store: HostStore) {
        self.store = store
        _model = State(initialValue: StreamModel(target: target, store: store))
    }

    var body: some View {
        ZStack(alignment: .topLeading) {
            Color.black.ignoresSafeArea()
            StreamViewRepresentable(
                model: model,
                touchMode: touchMode,
                keyboardRequests: keyboardRequests,
                controllerShown: controllerShown,
                menuOpen: menuOpen,
                openMenu: { menuOpen = true }
            )
            .ignoresSafeArea()
            status
            if showStats, let stats = model.stats {
                Text(stats.summary)
                    .font(.caption.monospacedDigit())
                    .padding(.horizontal, 10)
                    .padding(.vertical, 6)
                    .glassEffect(.regular, in: .capsule)
                    .padding(12)
                    .allowsHitTesting(false)
            }
            // Over the on-screen gamepad, the button moves to the middle,
            // clear of its shoulder buttons.
            menuButton
                .frame(maxWidth: .infinity, alignment: controllerShown ? .center : .trailing)
                .padding(12)
        }
        .statusBarHidden()
        .persistentSystemOverlays(.hidden)
        .defersSystemGestures(on: .all)
        .confirmationDialog(model.target.name, isPresented: $menuOpen, titleVisibility: .visible) {
            Button("Show Keyboard") { keyboardRequests += 1 }
            Button(controllerShown ? "Hide Controller" : "Show Controller") { controllerShown.toggle() }
            if TouchSource.current == .touchscreen {
                Button(touchMode == .trackpad ? "Use Direct Touch" : "Use Trackpad") {
                    touchMode = touchMode == .trackpad ? .direct : .trackpad
                }
            }
            Button(showStats ? "Hide Statistics" : "Show Statistics") { showStats.toggle() }
            Button("Disconnect", role: .destructive) { dismiss() }
        }
        .sheet(isPresented: pairing) {
            PairingView(hostName: model.target.name, retry: retry) { code in
                model.answerPairing(code)
            }
            .presentationDetents([.medium])
            .interactiveDismissDisabled()
        }
        .sheet(isPresented: signingIn) {
            GatewaySignInView(prompt: signInPrompt, signIn: model.answerSignIn, enterCode: model.answerSignInCode)
                .interactiveDismissDisabled()
        }
        .onAppear {
            store.streamStarted()
            UIApplication.shared.isIdleTimerDisabled = true
            Self.configureAudio()
        }
        .onDisappear {
            model.stop()
            store.streamEnded()
            UIApplication.shared.isIdleTimerDisabled = false
        }
        .onChange(of: model.phase) { _, phase in
            if phase == .ended { dismiss() }
        }
    }

    /// The button that opens the stream's menu, dimmed while streaming.
    private var menuButton: some View {
        Button {
            menuOpen = true
        } label: {
            Image(systemName: "ellipsis")
                .font(.title3.weight(.semibold))
                .frame(width: 44, height: 44)
        }
        .buttonStyle(.plain)
        .glassEffect(.regular.interactive(), in: .circle)
        .opacity(model.phase == .streaming ? 0.6 : 1)
        .accessibilityLabel("Stream menu")
    }

    /// What the stream is doing until its picture shows, or why it ended.
    @ViewBuilder private var status: some View {
        switch model.phase {
        case .connecting, .starting, .reconnecting:
            VStack(spacing: 12) {
                ProgressView()
                    .controlSize(.large)
                    .tint(.white)
                Text(statusText)
                    .foregroundStyle(.white)
                Button("Cancel") { dismiss() }
                    .buttonStyle(.bordered)
                    .tint(.white)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        case let .failed(error):
            ContentUnavailableView {
                Label("Couldn't Stream from \(model.target.name)", systemImage: "exclamationmark.triangle")
            } description: {
                Text(error)
            } actions: {
                Button("Close") { dismiss() }
                    .buttonStyle(.borderedProminent)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(.black)
            .environment(\.colorScheme, .dark)
        case .pairing, .signingIn, .streaming, .ended:
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

    private var signingIn: Binding<Bool> {
        Binding(
            get: { if case .signingIn = model.phase { true } else { false } },
            set: { _ in }
        )
    }

    private var signInPrompt: GatewayPrompt {
        if case let .signingIn(prompt) = model.phase { prompt } else { GatewayPrompt(gateway: "", retry: false, code: false) }
    }

    /// The shortest audio buffer asked for, in seconds: the system's usual
    /// least.
    private static let audioBuffer: TimeInterval = 0.005

    /// Sets the audio session up for low-latency playback, and recording
    /// when the microphone is sent.
    private static func configureAudio() {
        let session = AVAudioSession.sharedInstance()
        let microphone = UserDefaults.standard.bool(forKey: SettingsKey.microphone)
        try? session.setCategory(
            microphone ? .playAndRecord : .playback,
            mode: .default,
            options: microphone ? [.defaultToSpeaker, .allowBluetoothA2DP] : []
        )
        try? session.setPreferredIOBufferDuration(audioBuffer)
        try? session.setActive(true)
    }
}

/// The UIKit view controller the stream draws into, with the user's touch,
/// keyboard and mouse input.
struct StreamViewRepresentable: UIViewControllerRepresentable {
    let model: StreamModel
    let touchMode: TouchMode
    let keyboardRequests: Int
    let controllerShown: Bool
    let menuOpen: Bool
    let openMenu: () -> Void

    func makeUIViewController(context: Context) -> StreamViewController {
        StreamViewController(model: model, touchMode: touchMode, openMenu: openMenu)
    }

    func updateUIViewController(_ controller: StreamViewController, context: Context) {
        controller.touchMode = touchMode
        controller.controllerShown = controllerShown
        controller.menuOpen = menuOpen
        controller.showKeyboard(request: keyboardRequests)
    }

    static func dismantleUIViewController(_ controller: StreamViewController, coordinator: ()) {
        controller.tearDown()
    }
}
