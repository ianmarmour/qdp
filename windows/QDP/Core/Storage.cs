using System.Text.Json;
using System.Text.Json.Serialization;

namespace Qdp.Core;

/// <summary>
/// Where the app keeps what it remembers: the device's key and the log,
/// which the core writes, and the hosts kept and settings, as JSON, in
/// %LOCALAPPDATA%\QDP. Tokens are kept apart, by <see cref="Tokens"/>.
/// </summary>
internal static class Storage
{
    /// <summary>The app's data directory.</summary>
    internal static string DataDirectory { get; } = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "QDP");

    /// <summary>The kept hosts' file.</summary>
    private static string HostsFile => Path.Combine(DataDirectory, "hosts.json");

    /// <summary>The settings' file.</summary>
    private static string SettingsFile => Path.Combine(DataDirectory, "settings.json");

    /// <summary>Makes the data directory if it does not exist.</summary>
    internal static void Prepare() => Directory.CreateDirectory(DataDirectory);

    /// <summary>Returns the kept hosts, most recently kept first.</summary>
    internal static List<KeptHost> LoadHosts() =>
        Load(HostsFile, StorageJson.Default.ListKeptHost) ?? [];

    /// <summary>Saves the kept hosts.</summary>
    internal static void SaveHosts(List<KeptHost> hosts) =>
        Save(HostsFile, hosts, StorageJson.Default.ListKeptHost);

    /// <summary>Returns the settings.</summary>
    internal static AppSettings LoadSettings() =>
        Load(SettingsFile, StorageJson.Default.AppSettings) ?? new AppSettings();

    /// <summary>Keeps the settings.</summary>
    internal static void SaveSettings(AppSettings settings) =>
        Save(SettingsFile, settings, StorageJson.Default.AppSettings);

    /// <summary>Reads a file's JSON, or returns null if it cannot.</summary>
    private static T? Load<T>(string file, System.Text.Json.Serialization.Metadata.JsonTypeInfo<T> type)
    {
        try
        {
            return File.Exists(file) ? JsonSerializer.Deserialize(File.ReadAllText(file), type) : default;
        }
        catch (Exception exception) when (exception is IOException or JsonException or UnauthorizedAccessException)
        {
            return default;
        }
    }

    /// <summary>
    /// Writes a value's JSON to a file, through a temporary file so that a
    /// crash never leaves half of it.
    /// </summary>
    private static void Save<T>(string file, T value, System.Text.Json.Serialization.Metadata.JsonTypeInfo<T> type)
    {
        try
        {
            Prepare();
            var temporary = file + ".tmp";
            File.WriteAllText(temporary, JsonSerializer.Serialize(value, type));
            File.Move(temporary, file, overwrite: true);
        }
        catch (Exception exception) when (exception is IOException or UnauthorizedAccessException)
        {
            // What could not be kept is kept the next time.
        }
    }
}

/// <summary>The JSON the app keeps, made at build time.</summary>
[JsonSourceGenerationOptions(
    WriteIndented = true,
    PropertyNamingPolicy = JsonKnownNamingPolicy.CamelCase,
    UseStringEnumConverter = true)]
[JsonSerializable(typeof(List<KeptHost>))]
[JsonSerializable(typeof(AppSettings))]
internal sealed partial class StorageJson : JsonSerializerContext
{
}
