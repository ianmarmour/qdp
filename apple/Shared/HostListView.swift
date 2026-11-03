import SwiftUI

/// The computers to stream from: those paired with, then those nearby, each
/// one tap away; or, while none is found, what to do about it.
struct HostListView: View {
    @Bindable var store: HostStore
    /// Streams from, or pairs with, a host.
    var open: (StreamTarget) -> Void
    /// Shows the sheet to add a host by its address.
    var addByAddress: () -> Void

    @State private var forgetting: PairedHost?
    @State private var forgettingGatewayHost: GatewayHost?
    @State private var explanation: HostRow?
    @AppStorage(SettingsKey.gatewayAddress) private var gatewayAddress = ""
    @Environment(\.colorScheme) private var scheme

    var body: some View {
        let rows = store.rows
        let yours = rows.filter { $0.paired != nil }
        let nearby = rows.filter { $0.paired == nil }
        // The computers reached through the gateway, while one is set.
        let throughGateway = gatewayAddress.trimmingCharacters(in: .whitespaces).isEmpty ? [] : store.gatewayHosts
        Group {
            if rows.isEmpty && throughGateway.isEmpty {
                emptyState
            } else {
                List {
                    // The computers reached through the gateway are among
                    // yours, as any other.
                    if !yours.isEmpty || !throughGateway.isEmpty {
                        Section("Your Computers") {
                            ForEach(yours) { row in rowButton(row) }
                            ForEach(throughGateway) { host in gatewayButton(host) }
                        }
                    }
                    // A local network is searched while the list shows.
                    if store.networkAllowed && store.onLocalNetwork {
                        Section {
                            ForEach(nearby) { row in rowButton(row) }
                        } header: {
                            HStack(spacing: 6) {
                                Text("Nearby")
                                ProgressView()
                                    .controlSize(.mini)
                                    .accessibilityLabel("Searching")
                            }
                        }
                    }
                }
            }
        }
        .confirmationDialog(
            "Forget \(forgetting?.name ?? "this computer")?",
            isPresented: Binding(get: { forgetting != nil }, set: { if !$0 { forgetting = nil } }),
            titleVisibility: .visible,
            presenting: forgetting
        ) { host in
            Button("Forget", role: .destructive) { store.forget(pin: host.pin) }
        } message: { _ in
            Text("Connecting to it again needs pairing with a new code.")
        }
        .confirmationDialog(
            "Forget \(forgettingGatewayHost?.name ?? "this computer")?",
            isPresented: Binding(
                get: { forgettingGatewayHost != nil },
                set: { if !$0 { forgettingGatewayHost = nil } }
            ),
            titleVisibility: .visible,
            presenting: forgettingGatewayHost
        ) { host in
            Button("Forget", role: .destructive) { store.forgetGatewayHost(target: host.target) }
        } message: { _ in
            Text("You can add it again by its ID or name.")
        }
        .alert(
            explanation?.name ?? "",
            isPresented: Binding(get: { explanation != nil }, set: { if !$0 { explanation = nil } }),
            presenting: explanation
        ) { _ in
            Button("OK", role: .cancel) {}
        } message: { row in
            Text(Self.explanation(for: row))
        }
    }

    /// What to do while no computer is found, or the network cannot be
    /// searched.
    @ViewBuilder private var emptyState: some View {
        if !store.onLocalNetwork {
            ContentUnavailableView {
                Label("Not on a Local Network", systemImage: "wifi.slash")
            } description: {
                Text("Join Wi-Fi to find computers nearby, or add one by its address.")
            } actions: {
                Button("Add by Address", action: addByAddress)
            }
        } else if store.networkAllowed {
            ContentUnavailableView {
                Label {
                    Text("Looking for Computers")
                } icon: {
                    // The waves the system shows as it searches for Wi-Fi
                    // and AirDrop.
                    Image(systemName: "dot.radiowaves.left.and.right")
                        .symbolEffect(.variableColor.iterative.reversing)
                        .foregroundStyle(.tint)
                }
            } description: {
                Text("Run qdp serve on a computer on this network, and it appears here.")
            } actions: {
                Button(action: addByAddress) {
                    Text("Add by Address")
                        #if os(iOS)
                        .foregroundStyle(Brand.onAccent(scheme))
                        #endif
                }
                .buttonStyle(.borderedProminent)
            }
        } else {
            ContentUnavailableView {
                Label("Local Network Access Is Off", systemImage: "wifi.exclamationmark")
            } description: {
                Text("Allow QDP to find computers on your network in Privacy & Security settings, or add one by its address.")
            } actions: {
                Button("Add by Address", action: addByAddress)
            }
        }
    }

    private func rowButton(_ row: HostRow) -> some View {
        Button {
            if let target = row.target {
                open(target)
            } else {
                explanation = row
            }
        } label: {
            HStack(spacing: 14) {
                HostIcon(look: Self.look(for: row.status), symbol: Self.symbol(for: row.status))
                VStack(alignment: .leading, spacing: 2) {
                    Text(row.name)
                        .font(.body.weight(.medium))
                    if !row.detail.isEmpty {
                        Text(row.detail)
                            .font(.subheadline)
                            .foregroundStyle(.secondary)
                    }
                }
                Spacer()
                Self.trailing(for: row.status)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityHint(Self.hint(for: row.status))
        .contextMenu {
            if let paired = row.paired {
                Button("Forget", systemImage: "trash", role: .destructive) { forgetting = paired }
            }
        }
        #if os(iOS)
        .swipeActions {
            if let paired = row.paired {
                Button("Forget", systemImage: "trash", role: .destructive) { forgetting = paired }
            }
        }
        #endif
    }

    private func gatewayButton(_ host: GatewayHost) -> some View {
        Button {
            open(host.stream)
        } label: {
            HStack(spacing: 14) {
                HostIcon(look: .online, symbol: "desktopcomputer")
                Text(host.name)
                    .font(.body.weight(.medium))
                Spacer()
                Self.chevron
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityHint("Streams from this computer through your organization's gateway")
        .contextMenu {
            Button("Forget", systemImage: "trash", role: .destructive) { forgettingGatewayHost = host }
        }
        #if os(iOS)
        .swipeActions {
            Button("Forget", systemImage: "trash", role: .destructive) { forgettingGatewayHost = host }
        }
        #endif
    }

    private static func symbol(for status: HostRow.Status) -> String {
        switch status {
        case .ready, .notSeen, .resolving: "desktopcomputer"
        case .new: "plus"
        case .pairingOff: "lock.fill"
        case .differentKey: "exclamationmark.shield.fill"
        }
    }

    /// How a host's icon looks.
    private static func look(for status: HostRow.Status) -> HostIcon.Look {
        switch status {
        case .ready, .resolving: .online
        case .notSeen, .pairingOff: .offline
        case .new: .pairable
        case .differentKey: .warning
        }
    }

    /// What a row shows at its end: Pair for a computer to pair with, a
    /// chevron for one paired, and progress while one is found.
    @ViewBuilder private static func trailing(for status: HostRow.Status) -> some View {
        switch status {
        case .new:
            Text("Pair")
                .font(.body.weight(.semibold))
                .foregroundStyle(.tint)
        case .resolving:
            ProgressView()
                .controlSize(.small)
        case .ready, .notSeen:
            chevron
        case .pairingOff, .differentKey:
            EmptyView()
        }
    }

    /// The chevron at the end of a row that streams.
    private static var chevron: some View {
        Image(systemName: "chevron.right")
            .font(.footnote.weight(.semibold))
            .foregroundStyle(.tertiary)
    }

    private static func hint(for status: HostRow.Status) -> String {
        switch status {
        case .ready, .notSeen: "Streams from this computer"
        case .new: "Pairs with this computer"
        case .pairingOff, .differentKey, .resolving: ""
        }
    }

    private static func explanation(for row: HostRow) -> String {
        switch row.status {
        case .pairingOff:
            "Pairing is off on \(row.name). Start qdp serve there with pairing on to pair."
        case .differentKey:
            "This computer uses the name of one you paired with, but not its key, so QDP won't connect to it automatically."
        default:
            "\(row.name) can't be reached yet."
        }
    }
}
