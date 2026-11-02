import SwiftUI

/// Adds a computer by its address, for one the network search does not
/// find: pairing with it, or, under Advanced, connecting with its key pin
/// and a token it accepts. With the organization's gateway set, the
/// computer is reached through it by default, by its ID there or its name
/// inside the network.
struct AddHostView: View {
    /// Keeps a computer reached through the gateway.
    var keepGatewayHost: (GatewayHost) -> Void
    /// Opens the stream to the computer.
    var open: (StreamTarget) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var throughGateway = Gateway.address != nil

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
                    Text(throughGateway
                        ? "Its ID on your organization's gateway, or its name inside the organization's network."
                        : "Add :port if the computer serves on another port than 4433.")
                }
                if let gateway = Gateway.address {
                    Section {
                        Toggle("Through \(gateway)", isOn: $throughGateway)
                    } footer: {
                        Text("Your organization's gateway. Turn off for a computer on this network.")
                    }
                }
                if !throughGateway {
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
            }
            .formStyle(.grouped)
            .navigationTitle("Add Computer")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(throughGateway ? "Connect" : "Pair") {
                        if throughGateway {
                            let host = GatewayHost(name: trimmed, target: trimmed)
                            keepGatewayHost(host)
                            open(host.stream)
                        } else {
                            open(StreamTarget(name: trimmed, address: trimmed, authentication: .pair))
                        }
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
