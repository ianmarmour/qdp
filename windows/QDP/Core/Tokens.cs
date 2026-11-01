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
    /// <summary>The resource the tokens are kept under, which Credential Manager shows.</summary>
    private const string Resource = "QDP host token";

    /// <summary>What the Credential Locker fails with when it holds no such token.</summary>
    private const int ElementNotFound = unchecked((int)0x80070490);

    /// <summary>
    /// Keeps <paramref name="token"/> for the host whose key pin is
    /// <paramref name="pin"/>, in place of any kept before, returning
    /// whether it could.
    /// </summary>
    internal static bool Keep(string pin, string token)
    {
        try
        {
            var vault = new PasswordVault();
            Remove(vault, pin);
            vault.Add(new PasswordCredential(Resource, pin, token));
            return true;
        }
        catch (Exception exception) when (IsLockerFailure(exception))
        {
            return false;
        }
    }

    /// <summary>Returns the token kept for the host whose key pin is <paramref name="pin"/>, or null.</summary>
    internal static string? Find(string pin)
    {
        try
        {
            var credential = new PasswordVault().Retrieve(Resource, pin);
            credential.RetrievePassword();
            return credential.Password;
        }
        catch (Exception exception) when (IsLockerFailure(exception))
        {
            return null;
        }
    }

    /// <summary>Forgets the token kept for the host whose key pin is <paramref name="pin"/>, if any.</summary>
    internal static void Forget(string pin)
    {
        try
        {
            Remove(new PasswordVault(), pin);
        }
        catch (Exception exception) when (IsLockerFailure(exception))
        {
            // A token that cannot be removed is never used again: its host
            // is no longer kept.
        }
    }

    /// <summary>Removes the token kept in <paramref name="vault"/> for <paramref name="pin"/>, if any.</summary>
    private static void Remove(PasswordVault vault, string pin)
    {
        try
        {
            vault.Remove(vault.Retrieve(Resource, pin));
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
