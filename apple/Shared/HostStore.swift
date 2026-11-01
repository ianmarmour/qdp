import Foundation
import Network
import Observation

/// A host on the local network: its service name, its address and port once
/// resolved, the key pin it claims and whether it offers pairing. Nothing
/// here is authenticated: the pin only recognizes a host already paired
/// with, which the connection then verifies (Section 3.13).
struct DiscoveredHost: Hashable, Identifiable {
    var name: String
    var address: String?
    var pin: String?
    var pairing: Bool

    var id: String { name }
}

/// A host the list shows, and what choosing it does.
struct HostRow: Identifiable, Hashable {
    /// How the row stands.
    enum Status: Hashable {
        /// Paired and on this network: choosing it streams.
        case ready
        /// Paired, but not seen on this network: choosing it tries anyway.
        case notSeen
        /// New and offering pairing: choosing it pairs.
        case new
        /// New, with pairing off.
        case pairingOff
        /// Found under a paired host's name with another key, which may pose
        /// as it.
        case differentKey
        /// Found, but its address is not known yet.
        case resolving
    }

    var name: String
    var detail: String
    var status: Status
    var target: StreamTarget?
    var paired: PairedHost?

    var id: String { "\(name)|\(paired?.pin ?? "")|\(status)" }
}

/// The hosts paired with, kept between launches, and those on the local
/// network, combined into the list the app shows.
@MainActor
@Observable
final class HostStore {
    /// The hosts paired with, most recently kept first.
    private(set) var paired: [PairedHost] = []
    /// The hosts on the local network.
    private(set) var discovered: [DiscoveredHost] = []
    /// Whether the local network can be browsed; `false` once the user
    /// declined the permission.
    private(set) var networkAllowed = true

    private let defaults: UserDefaults
    private var browser: NWBrowser?
    private var resolvers: [String: NWConnection] = [:]
    /// Whether the host list shows.
    private var listShown = false
    /// The streams running, during which the network is not searched.
    private var streams = 0

    /// Where paired hosts are kept.
    private static let pairedKey = "pairedHosts"
    /// The service type of QDP hosts.
    private static let serviceType = "_qdp._udp"
    /// The TXT key of the host's key pin.
    private static let pinKey = "pin"
    /// The TXT key present while the host offers pairing.
    private static let pairKey = "pair"
    /// DNS-SD's `kDNSServiceErr_PolicyDenied`: the user declined the local
    /// network permission.
    private static let policyDenied: Int32 = -65570

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        if let data = defaults.data(forKey: Self.pairedKey),
           let hosts = try? JSONDecoder().decode([PairedHost].self, from: data) {
            paired = hosts
        }
    }

    /// Keeps `host`, replacing the host with its pin.
    func keep(_ host: PairedHost) {
        paired = [host] + paired.filter { $0.pin != host.pin }
        save()
    }

    /// Forgets the host whose pin is `pin`.
    func forget(pin: String) {
        paired.removeAll { $0.pin == pin }
        save()
    }

    private func save() {
        if let data = try? JSONEncoder().encode(paired) {
            defaults.set(data, forKey: Self.pairedKey)
        }
    }

    /// The rows to list: hosts found on the network, recognized as paired by
    /// their pin, then paired hosts not found.
    var rows: [HostRow] {
        var found = Set<String>()
        let nearby = discovered.sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
        var rows: [HostRow] = nearby.map { host in
            let match = paired.first { $0.pin == host.pin }
            guard let address = host.address else {
                return HostRow(name: host.name, detail: "Finding its address…", status: .resolving)
            }
            if let match {
                found.insert(match.pin)
                return HostRow(
                    name: host.name,
                    detail: "Ready · \(address)",
                    status: .ready,
                    target: StreamTarget(name: host.name, address: address, authentication: .paired, pin: match.pin),
                    paired: match
                )
            }
            if paired.contains(where: { $0.name == host.name }) {
                return HostRow(
                    name: host.name,
                    detail: "This isn't the \(host.name) you paired with",
                    status: .differentKey,
                    target: host.pairing ? StreamTarget(name: host.name, address: address, authentication: .pair) : nil
                )
            }
            if host.pairing {
                return HostRow(
                    name: host.name,
                    detail: "New · \(address)",
                    status: .new,
                    target: StreamTarget(name: host.name, address: address, authentication: .pair)
                )
            }
            return HostRow(name: host.name, detail: "Pairing is off on this computer", status: .pairingOff)
        }
        rows += paired.filter { !found.contains($0.pin) }.map { host in
            HostRow(
                name: host.name,
                detail: "Not on this network · \(host.address)",
                status: .notSeen,
                target: StreamTarget(name: host.name, address: host.address, authentication: .paired, pin: host.pin),
                paired: host
            )
        }
        return rows
    }

    /// Notes that the host list shows, which searches the network unless a
    /// stream runs.
    func windowAppeared() {
        listShown = true
        updateBrowsing()
    }

    /// Notes that the host list no longer shows.
    func windowDisappeared() {
        listShown = false
        updateBrowsing()
    }

    /// Notes that a stream started: searching the network makes Wi-Fi
    /// stutter, so it stops while streams run.
    func streamStarted() {
        streams += 1
        updateBrowsing()
    }

    /// Notes that a stream ended.
    func streamEnded() {
        streams = max(streams - 1, 0)
        updateBrowsing()
    }

    private func updateBrowsing() {
        if listShown && streams == 0 {
            startBrowsing()
        } else {
            stopBrowsing()
        }
    }

    /// Starts looking for hosts on the local network.
    func startBrowsing() {
        guard browser == nil else { return }
        let browser = NWBrowser(for: .bonjourWithTXTRecord(type: Self.serviceType, domain: nil), using: .udp)
        browser.stateUpdateHandler = { [weak self] state in
            Task { @MainActor in self?.browserChanged(state) }
        }
        browser.browseResultsChangedHandler = { [weak self] results, _ in
            Task { @MainActor in self?.resultsChanged(results) }
        }
        browser.start(queue: .main)
        self.browser = browser
    }

    /// Stops looking for hosts, as while streaming, when browsing disturbs
    /// Wi-Fi.
    func stopBrowsing() {
        browser?.cancel()
        browser = nil
        resolvers.values.forEach { $0.cancel() }
        resolvers.removeAll()
    }

    private func browserChanged(_ state: NWBrowser.State) {
        switch state {
        case .ready:
            networkAllowed = true
        case let .waiting(error), let .failed(error):
            if case let .dns(code) = error, code == Self.policyDenied {
                networkAllowed = false
            }
        default:
            break
        }
    }

    private func resultsChanged(_ results: Set<NWBrowser.Result>) {
        var hosts: [DiscoveredHost] = []
        for result in results {
            guard case let .service(name, _, _, _) = result.endpoint else { continue }
            var pin: String?
            var pairing = false
            if case let .bonjour(record) = result.metadata {
                pin = record[Self.pinKey]?.lowercased()
                pairing = record[Self.pairKey] == "1"
            }
            let known = discovered.first { $0.name == name }
            hosts.append(DiscoveredHost(
                name: name,
                address: known?.address,
                pin: pin.flatMap { Core.isPin($0) ? $0 : nil },
                pairing: pairing
            ))
            if known?.address == nil {
                resolve(name: name, endpoint: result.endpoint)
            }
        }
        discovered = hosts
    }

    /// Finds the address and port of a service, as the core connects to
    /// them, by opening a UDP connection to it and reading where it went.
    private func resolve(name: String, endpoint: NWEndpoint) {
        guard resolvers[name] == nil else { return }
        let parameters = NWParameters.udp
        if let options = parameters.defaultProtocolStack.internetProtocol as? NWProtocolIP.Options {
            options.version = .v4
        }
        let connection = NWConnection(to: endpoint, using: parameters)
        resolvers[name] = connection
        connection.stateUpdateHandler = { [weak self, weak connection] state in
            guard case .ready = state, let connection else { return }
            let remote = connection.currentPath?.remoteEndpoint
            Task { @MainActor in
                self?.resolved(name: name, endpoint: remote)
                connection.cancel()
            }
        }
        connection.start(queue: .main)
    }

    private func resolved(name: String, endpoint: NWEndpoint?) {
        resolvers[name] = nil
        guard case let .hostPort(host, port) = endpoint else { return }
        let address: String
        switch host {
        case let .ipv4(ip):
            address = "\(ip):\(port.rawValue)"
        case let .ipv6(ip):
            address = "[\(ip)]:\(port.rawValue)"
        case let .name(hostName, _):
            address = "\(hostName):\(port.rawValue)"
        @unknown default:
            return
        }
        if let index = discovered.firstIndex(where: { $0.name == name }) {
            discovered[index].address = address
        }
    }
}
