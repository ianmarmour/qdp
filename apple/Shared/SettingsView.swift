import SwiftUI

/// How streams look and behave, kept between launches.
struct SettingsView: View {
    @AppStorage(SettingsKey.resolution) private var resolution = Resolution.window
    @AppStorage(SettingsKey.decoder) private var decoder = DecoderChoice.automatic
    @AppStorage(SettingsKey.touchMode) private var touchMode = TouchMode.trackpad
    @AppStorage(SettingsKey.showStats) private var showStats = false
    @AppStorage(SettingsKey.microphone) private var microphone = false

    @State private var clientPin: String?

    var body: some View {
        Form {
            Section("Stream") {
                Picker("Resolution", selection: $resolution) {
                    ForEach(Resolution.allCases) { Text($0.label).tag($0) }
                }
                Toggle("Send the Microphone", isOn: $microphone)
                Toggle("Show Statistics", isOn: $showStats)
            }
            #if os(iOS)
            // A Mac running the iPhone and iPad app has no touchscreen.
            if TouchSource.current == .touchscreen {
                Section {
                    Picker("Touch", selection: $touchMode) {
                        ForEach(TouchMode.allCases) { Text($0.label).tag($0) }
                    }
                    .pickerStyle(.segmented)
                } header: {
                    Text("Touch")
                } footer: {
                    Text(touchMode.explanation)
                }
            }
            #endif
            Section {
                Picker("Decoder", selection: $decoder) {
                    ForEach(DecoderChoice.allCases) { Text($0.label).tag($0) }
                }
            } header: {
                Text("Advanced")
            } footer: {
                Text("Automatic decodes on the graphics hardware where this device can, and in software otherwise.")
            }
            Section {
                if let clientPin {
                    LabeledContent("Key pin") {
                        Text(clientPin)
                            .font(.caption.monospaced())
                            .textSelection(.enabled)
                            .lineLimit(2)
                    }
                }
            } header: {
                Text("This Device")
            } footer: {
                Text("A computer accepts this device without pairing when started with qdp serve --client-key and this pin.")
            }
        }
        .formStyle(.grouped)
        .task { clientPin = Core.clientPin }
        #if os(macOS)
        .frame(width: 520)
        .padding(.vertical, 8)
        #endif
    }
}
