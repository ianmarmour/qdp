import SwiftUI

/// Adds a computer by its address, for one the network search does not
/// find: pairing with it, or, under Advanced, connecting with its key pin
/// and a token it accepts.
struct AddHostView: View {
    /// Opens the stream to the computer.
    var open: (StreamTarget) -> Void
    @Environment(\.dismiss) private var dismiss

    @State private var address = ""
    @State private var pin = ""
    @State private var token = ""
    @State private var advanced = false

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("Address", text: $address, prompt: Text("192.168.1.20 or desk.local"))
                        .autocorrectionDisabled()
                        #if os(iOS)
                        .textInputAutocapitalization(.never)
                        .keyboardType(.URL)
                        #endif
                } footer: {
                    Text("Add :port if the computer serves on another port than 4433.")
                }
                Section {
                    DisclosureGroup("Advanced", isExpanded: $advanced) {
                        TextField("Key pin", text: $pin, prompt: Text("64 digits from qdp pin"))
                            .autocorrectionDisabled()
                            .font(.body.monospaced())
                        SecureField("Token", text: $token)
                        Button("Connect with Token") {
                            open(StreamTarget(name: trimmed, address: trimmed, authentication: .token, pin: pinText, token: token))
                            dismiss()
                        }
                        .disabled(trimmed.isEmpty || !Core.isPin(pinText) || token.isEmpty)
                    }
                }
            }
            .formStyle(.grouped)
            .navigationTitle("Add Computer")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Pair") {
                        open(StreamTarget(name: trimmed, address: trimmed, authentication: .pair))
                        dismiss()
                    }
                    .disabled(trimmed.isEmpty)
                }
            }
        }
        #if os(macOS)
        .frame(minWidth: 420, minHeight: 320)
        #endif
    }

    private var trimmed: String { address.trimmingCharacters(in: .whitespaces) }
    private var pinText: String { pin.trimmingCharacters(in: .whitespaces) }
}
