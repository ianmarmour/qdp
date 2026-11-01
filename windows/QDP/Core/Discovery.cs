using System.Net;
using System.Net.Sockets;
using Microsoft.UI.Dispatching;
using Windows.Devices.Enumeration;

namespace Qdp.Core;

/// <summary>
/// A host on the local network: its service name, its address and port once
/// resolved, the key pin it claims and whether it offers pairing. Nothing
/// here is authenticated: the pin only recognizes a host already paired
/// with, which the connection then verifies (Section 3.13).
/// </summary>
/// <param name="Name">Its DNS-SD instance name.</param>
/// <param name="Address">Its address and port, once resolved.</param>
/// <param name="Pin">The key pin it claims, if any.</param>
/// <param name="Pairing">Whether it offers pairing.</param>
internal sealed record DiscoveredHost(string Name, string? Address, string? Pin, bool Pairing);

/// <summary>
/// Finds QDP hosts on the local network through Windows' DNS-SD browser,
/// telling of them on the UI thread.
/// </summary>
internal sealed class Discovery
{
    /// <summary>
    /// The DNS-SD services of QDP hosts in the domain <c>local.</c>, as
    /// Windows' device enumeration asks for them.
    /// </summary>
    private const string Query =
        "System.Devices.AepService.ProtocolId:={4526e8c1-8aac-4153-9b16-55e86ada0e54} AND " +
        "System.Devices.Dnssd.Domain:=\"local\" AND System.Devices.Dnssd.ServiceName:=\"_qdp._udp\"";

    /// <summary>The service's instance name.</summary>
    private const string InstanceNameProperty = "System.Devices.Dnssd.InstanceName";

    /// <summary>The host's name, for when it has no address yet.</summary>
    private const string HostNameProperty = "System.Devices.Dnssd.HostName";

    /// <summary>The service's port.</summary>
    private const string PortProperty = "System.Devices.Dnssd.PortNumber";

    /// <summary>The service's TXT record, as <c>key=value</c> strings.</summary>
    private const string TextProperty = "System.Devices.Dnssd.TextAttributes";

    /// <summary>The host's addresses.</summary>
    private const string AddressProperty = "System.Devices.IpAddress";

    /// <summary>The TXT key of the host's key pin.</summary>
    private const string PinKey = "pin";

    /// <summary>The TXT key present while the host offers pairing.</summary>
    private const string PairKey = "pair";

    /// <summary>The value of <see cref="PairKey"/> while the host offers pairing.</summary>
    private const string PairOffered = "1";

    /// <summary>The properties asked for with each service.</summary>
    private static readonly string[] Properties =
        [InstanceNameProperty, HostNameProperty, PortProperty, TextProperty, AddressProperty];

    /// <summary>Where the app hears of hosts.</summary>
    private readonly DispatcherQueue dispatcher;

    /// <summary>The services found, by their device identifier.</summary>
    private readonly Dictionary<string, DeviceInformation> services = [];

    /// <summary>The browser, while it runs.</summary>
    private DeviceWatcher? watcher;

    /// <summary>Returns a browser that tells of hosts on <paramref name="dispatcher"/>.</summary>
    internal Discovery(DispatcherQueue dispatcher) => this.dispatcher = dispatcher;

    /// <summary>Raised on the UI thread when the hosts found change.</summary>
    internal event Action<IReadOnlyList<DiscoveredHost>>? Changed;

    /// <summary>Starts looking for hosts, if it is not already.</summary>
    internal void Start()
    {
        if (watcher is not null)
        {
            return;
        }
        try
        {
            watcher = DeviceInformation.CreateWatcher(
                Query, Properties, DeviceInformationKind.AssociationEndpointService);
        }
        catch (Exception exception) when (exception is ArgumentException or System.Runtime.InteropServices.COMException)
        {
            // Without a browser, hosts are added by address.
            return;
        }
        watcher.Added += (_, service) => OnUi(() =>
        {
            services[service.Id] = service;
            Publish();
        });
        watcher.Updated += (_, update) => OnUi(() =>
        {
            if (services.TryGetValue(update.Id, out var service))
            {
                service.Update(update);
                Publish();
            }
        });
        watcher.Removed += (_, update) => OnUi(() =>
        {
            if (services.Remove(update.Id))
            {
                Publish();
            }
        });
        watcher.Start();
    }

    /// <summary>
    /// Stops looking for hosts, as while streaming, when querying the
    /// network can make Wi-Fi stutter.
    /// </summary>
    internal void Stop()
    {
        if (watcher is null)
        {
            return;
        }
        if (watcher.Status is DeviceWatcherStatus.Started or DeviceWatcherStatus.EnumerationCompleted)
        {
            watcher.Stop();
        }
        watcher = null;
        services.Clear();
        Publish();
    }

    /// <summary>Runs <paramref name="action"/> on the UI thread.</summary>
    private void OnUi(Action action) => dispatcher.TryEnqueue(() => action());

    /// <summary>Tells the app of the hosts found.</summary>
    private void Publish() => Changed?.Invoke(services.Values.Select(Host).ToList());

    /// <summary>Returns what a service says of its host.</summary>
    private static DiscoveredHost Host(DeviceInformation service)
    {
        var properties = service.Properties;
        var name = properties.TryGetValue(InstanceNameProperty, out var instance) && instance is string text
            ? text
            : service.Name;
        var attributes = properties.TryGetValue(TextProperty, out var record) && record is string[] strings
            ? strings
            : [];
        var values = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        foreach (var attribute in attributes)
        {
            var separator = attribute.IndexOf('=');
            if (separator > 0)
            {
                values[attribute[..separator]] = attribute[(separator + 1)..];
            }
        }
        var pin = values.TryGetValue(PinKey, out var claimed) ? claimed.ToLowerInvariant() : null;
        return new DiscoveredHost(
            name,
            Address(properties),
            pin is not null && Native.qdp_is_pin(pin) ? pin : null,
            values.TryGetValue(PairKey, out var pairing) && pairing == PairOffered);
    }

    /// <summary>
    /// Returns a service's address and port: its first IPv4 address, else
    /// its first IPv6 one, else its host name; or null before it resolved.
    /// </summary>
    private static string? Address(IReadOnlyDictionary<string, object> properties)
    {
        var port = properties.TryGetValue(PortProperty, out var portValue)
            ? portValue switch
            {
                ushort value => value,
                uint value when value <= ushort.MaxValue => (ushort)value,
                int value when value is > 0 and <= ushort.MaxValue => (ushort)value,
                _ => (ushort)0,
            }
            : (ushort)0;
        if (port == 0)
        {
            return null;
        }
        var addresses = (properties.TryGetValue(AddressProperty, out var list) && list is string[] found ? found : [])
            .Select(text => IPAddress.TryParse(text, out var address) ? address : null)
            .OfType<IPAddress>()
            .ToList();
        var chosen = addresses.FirstOrDefault(address => address.AddressFamily == AddressFamily.InterNetwork)
            ?? addresses.FirstOrDefault();
        if (chosen is not null)
        {
            return new IPEndPoint(chosen, port).ToString();
        }
        return properties.TryGetValue(HostNameProperty, out var hostName) && hostName is string name && name.Length > 0
            ? $"{name.TrimEnd('.')}:{port}"
            : null;
    }
}
