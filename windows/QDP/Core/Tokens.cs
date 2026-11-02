using System.Globalization;
using System.Runtime.InteropServices;
using Windows.Security.Credentials;

namespace Qdp.Core;

/// <summary>
/// The tokens of the hosts kept with one, in Windows' Credential Locker,
/// which keeps them encrypted for the user signed in, rather than in the
/// app's files; each is kept under its host's key pin.
/// </summary>
internal static class Tokens
{
    /// <summary>Where the tokens are kept, under the resource Credential Manager shows.</summary>
    private static readonly Locker Locker = new("QDP host token");

    /// <summary>
    /// Keeps <paramref name="token"/> for the host whose key pin is
    /// <paramref name="pin"/>, in place of any kept before, returning
    /// whether it could.
    /// </summary>
    internal static bool Keep(string pin, string token) => Locker.Keep(pin, token);

    /// <summary>Returns the token kept for the host whose key pin is <paramref name="pin"/>, or null.</summary>
    internal static string? Find(string pin) => Locker.Find(pin);

    /// <summary>Forgets the token kept for the host whose key pin is <paramref name="pin"/>, if any.</summary>
    internal static void Forget(string pin) => Locker.Forget(pin);
}

/// <summary>
/// The session tokens of the gateways the user signed in to, in Windows'
/// Credential Locker, each under its gateway's address with when it
/// expires, so that later sessions sign in without asking until then.
/// </summary>
internal static class GatewaySessions
{
    /// <summary>Where the tokens are kept, under the resource Credential Manager shows.</summary>
    private static readonly Locker Locker = new("QDP gateway session");

    /// <summary>What separates when a token expires from the token in what the locker keeps.</summary>
    private const char Separator = ' ';

    /// <summary>
    /// Keeps <paramref name="token"/> for the gateway at
    /// <paramref name="address"/>, valid for <paramref name="lifetimeSeconds"/>,
    /// in place of its last.
    /// </summary>
    internal static void Keep(string address, string token, long lifetimeSeconds)
    {
        var expires = DateTimeOffset.UtcNow.ToUnixTimeSeconds() + lifetimeSeconds;
        // A token that cannot be kept only means signing in again.
        Locker.Keep(address, expires.ToString(CultureInfo.InvariantCulture) + Separator + token);
    }

    /// <summary>Returns the token kept for the gateway at <paramref name="address"/>, if it has not expired.</summary>
    internal static string? Find(string address)
    {
        var kept = Locker.Find(address)?.Split(Separator);
        if (kept is not [var expires, var token]
            || !long.TryParse(expires, NumberStyles.Integer, CultureInfo.InvariantCulture, out var expiry)
            || expiry <= DateTimeOffset.UtcNow.ToUnixTimeSeconds())
        {
            return null;
        }
        return token;
    }
}

/// <summary>
/// Secrets of one kind in Windows' Credential Locker, under a resource of
/// their own, each under a name.
/// </summary>
/// <param name="resource">The resource they are kept under, which Credential Manager shows.</param>
internal sealed class Locker(string resource)
{
    /// <summary>What the Credential Locker fails with when it holds no such secret.</summary>
    private const int ElementNotFound = unchecked((int)0x80070490);

    /// <summary>
    /// Keeps <paramref name="secret"/> under <paramref name="name"/>, in place
    /// of any kept before, returning whether it could.
    /// </summary>
    internal bool Keep(string name, string secret)
    {
        try
        {
            var vault = new PasswordVault();
            Remove(vault, name);
            vault.Add(new PasswordCredential(resource, name, secret));
            return true;
        }
        catch (Exception exception) when (IsLockerFailure(exception))
        {
            return false;
        }
    }

    /// <summary>Returns the secret kept under <paramref name="name"/>, or null.</summary>
    internal string? Find(string name)
    {
        try
        {
            var credential = new PasswordVault().Retrieve(resource, name);
            credential.RetrievePassword();
            return credential.Password;
        }
        catch (Exception exception) when (IsLockerFailure(exception))
        {
            return null;
        }
    }

    /// <summary>Forgets the secret kept under <paramref name="name"/>, if any.</summary>
    internal void Forget(string name)
    {
        try
        {
            Remove(new PasswordVault(), name);
        }
        catch (Exception exception) when (IsLockerFailure(exception))
        {
            // A secret that cannot be removed is never used again: what it
            // was kept for is no longer kept.
        }
    }

    /// <summary>Removes the secret kept in <paramref name="vault"/> under <paramref name="name"/>, if any.</summary>
    private void Remove(PasswordVault vault, string name)
    {
        try
        {
            vault.Remove(vault.Retrieve(resource, name));
        }
        catch (COMException exception) when (exception.HResult == ElementNotFound)
        {
            // None was kept.
        }
    }

    /// <summary>Returns whether <paramref name="exception"/> is the Credential Locker failing.</summary>
    private static bool IsLockerFailure(Exception exception) =>
        exception is COMException or UnauthorizedAccessException;
}
