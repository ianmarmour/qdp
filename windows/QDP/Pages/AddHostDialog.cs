using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Qdp.Core;

namespace Qdp.Pages;

/// <summary>
/// Asks for a host by its address, for one the network does not show:
/// pairing with the code it shows, or with its key pin and a token; or, with
/// the organization's gateway set, by what the gateway knows it as, which it
/// is reached through by default.
/// </summary>
internal static class AddHostDialog
{
    /// <summary>What the address asks for when the host is reached directly.</summary>
    private const string DirectPlaceholder = "192.168.1.20, or my-pc.local";

    /// <summary>What the address asks for when the host is reached through the gateway.</summary>
    private const string GatewayPlaceholder = "Its ID at the gateway, or its name inside the network";

    /// <summary>
    /// Returns the stream the user asked for, or null if they cancelled;
    /// through <paramref name="gateway"/> if it is set and the user left it
    /// on.
    /// </summary>
    internal static async Task<StreamTarget?> Ask(XamlRoot root, GatewaySettings gateway)
    {
        var through = new ToggleSwitch
        {
            Header = $"Through your organization's gateway, {gateway.Address.Trim()}",
            IsOn = gateway.IsSet,
            Visibility = gateway.IsSet ? Visibility.Visible : Visibility.Collapsed,
        };
        var address = new TextBox
        {
            Header = "Address",
            PlaceholderText = DirectPlaceholder,
        };
        var name = new TextBox
        {
            Header = "Name (optional)",
            PlaceholderText = "The name this PC shows it by",
        };
        var method = new RadioButtons { Header = "Connect by", SelectedIndex = 0 };
        method.Items.Add("Pairing with the code the computer shows");
        method.Items.Add("The computer's key pin and a token");
        var pin = new TextBox
        {
            Header = "Key pin",
            PlaceholderText = "64 hexadecimal digits, from qdp host",
            Visibility = Visibility.Collapsed,
        };
        var token = new PasswordBox
        {
            Header = "Token",
            Visibility = Visibility.Collapsed,
        };
        var content = new StackPanel { Spacing = 12, MinWidth = 360 };
        foreach (var element in new UIElement[] { through, address, name, method, pin, token })
        {
            content.Children.Add(element);
        }
        var dialog = new ContentDialog
        {
            XamlRoot = root,
            Title = "Add a computer",
            Content = content,
            PrimaryButtonText = "Pair",
            CloseButtonText = "Cancel",
            DefaultButton = ContentDialogButton.Primary,
            IsPrimaryButtonEnabled = false,
        };

        bool ThroughGateway() => through.IsOn;
        bool UsesToken() => !ThroughGateway() && method.SelectedIndex == 1;
        void Update()
        {
            var tokenShown = UsesToken() ? Visibility.Visible : Visibility.Collapsed;
            pin.Visibility = tokenShown;
            token.Visibility = tokenShown;
            method.Visibility = ThroughGateway() ? Visibility.Collapsed : Visibility.Visible;
            address.Header = ThroughGateway() ? "Computer" : "Address";
            address.PlaceholderText = ThroughGateway() ? GatewayPlaceholder : DirectPlaceholder;
            dialog.PrimaryButtonText = UsesToken() || ThroughGateway() ? "Connect" : "Pair";
            dialog.IsPrimaryButtonEnabled = address.Text.Trim().Length > 0
                && (!UsesToken() || (Native.qdp_is_pin(pin.Text.Trim().ToLowerInvariant()) && token.Password.Length > 0));
        }
        address.TextChanged += (_, _) => Update();
        pin.TextChanged += (_, _) => Update();
        token.PasswordChanged += (_, _) => Update();
        method.SelectionChanged += (_, _) => Update();
        through.Toggled += (_, _) => Update();
        Update();

        if (await dialog.ShowAsync() != ContentDialogResult.Primary)
        {
            return null;
        }
        var host = address.Text.Trim();
        var title = name.Text.Trim().Length > 0 ? name.Text.Trim() : host;
        if (ThroughGateway())
        {
            return new StreamTarget(title, host, Authentication.Gateway);
        }
        return UsesToken()
            ? new StreamTarget(title, host, Authentication.Token, pin.Text.Trim().ToLowerInvariant(), token.Password)
            : new StreamTarget(title, host, Authentication.Pair);
    }
}
