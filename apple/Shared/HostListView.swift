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
                    if !yours.isEmpty {
                        Section("Your Computers") {
                            ForEach(yours) { row in rowButton(row) }
                        }
                    }
                    if !throughGateway.isEmpty {
                        Section("Through \(gatewayAddress)") {
                            ForEach(throughGateway) { host in gatewayButton(host) }
                        }
                    }
                    if !nearby.isEmpty {
                        Section("Nearby") {
                            ForEach(nearby) { row in rowButton(row) }
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
        if store.networkAllowed {
            ContentUnavailableView {
                Label("Looking for Computers", systemImage: "desktopcomputer")
            } description: {
                Text("Run qdp serve on a computer on this network, and it appears here.")
            } actions: {
                Button("Add by Address", action: addByAddress)
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
            HStack(spacing: 12) {
                Image(systemName: Self.symbol(for: row.status))
                    .font(.title2)
                    .foregroundStyle(Self.tint(for: row.status))
                    .frame(width: 32)
                VStack(alignment: .leading, spacing: 2) {
                    Text(row.name)
                        .font(.headline)
                    Text(row.detail)
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }
                Spacer()
                if row.status == .new {
                    Text("Pair")
                        .font(.subheadline.weight(.semibold))
                        .foregroundStyle(.tint)
                } else if row.status == .resolving {
                    ProgressView()
                        .controlSize(.small)
                }
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
            HStack(spacing: 12) {
                Image(systemName: "desktopcomputer")
                    .font(.title2)
                    .foregroundStyle(.tint)
                    .frame(width: 32)
                VStack(alignment: .leading, spacing: 2) {
                    Text(host.name)
                        .font(.headline)
                    Text(host.target)
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }
                Spacer()
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
        case .ready: "desktopcomputer"
        case .notSeen: "desktopcomputer.trianglebadge.exclamationmark"
        case .new: "plus.circle"
        case .pairingOff: "lock"
        case .differentKey: "exclamationmark.shield"
        case .resolving: "desktopcomputer"
        }
    }

    private static func tint(for status: HostRow.Status) -> Color {
        switch status {
        case .ready: .accentColor
        case .differentKey: .orange
        default: .secondary
        }
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
