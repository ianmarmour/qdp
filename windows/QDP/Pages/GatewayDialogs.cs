using Microsoft.UI;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Qdp.Core;
using Windows.ApplicationModel.DataTransfer;
using Windows.Foundation;

namespace Qdp.Pages;

/// <summary>The user's name and password for the organization's directory.</summary>
/// <param name="User">The user's name.</param>
/// <param name="Password">Their password.</param>
internal sealed record SignInCredentials(string User, string Password);

/// <summary>
/// Signing in to the organization's gateway (Section 13 of the
/// specification): the user's name and password, then a code from their
/// authenticator app, with the QR code that adds the gateway to it the first
/// time.
/// </summary>
internal static class GatewayDialogs
{
    /// <summary>The longest code an authenticator app shows.</summary>
    private const int MaxCodeLength = 10;

    /// <summary>
    /// Asks for the user's name, as <paramref name="user"/> if set, and
    /// password for <paramref name="gateway"/>, returning them, or null if the
    /// user declined; <paramref name="opened"/> is told of the dialog, which
    /// the app hides if the stream ends.
    /// </summary>
    internal static async Task<SignInCredentials?> AskSignIn(
        XamlRoot root, string gateway, bool retry, string user, Action<ContentDialog?> opened)
    {
        var name = new TextBox
        {
            Header = "User name",
            PlaceholderText = "Your name in your organization's directory",
            Text = user,
        };
        var password = new PasswordBox { Header = "Password" };
        var content = new StackPanel { Spacing = 12, MinWidth = 360 };
        content.Children.Add(Explanation(retry
            ? "That user name or password didn't work. Try again."
            : $"Sign in with your organization's account to reach its computers through {gateway}."));
        content.Children.Add(name);
        content.Children.Add(password);
        var dialog = new ContentDialog
        {
            XamlRoot = root,
            Title = $"Sign in to {gateway}",
            Content = content,
            PrimaryButtonText = "Sign in",
            CloseButtonText = "Cancel",
            DefaultButton = ContentDialogButton.Primary,
            IsPrimaryButtonEnabled = false,
        };
        void Update() => dialog.IsPrimaryButtonEnabled = name.Text.Trim().Length > 0 && password.Password.Length > 0;
        name.TextChanged += (_, _) => Update();
        password.PasswordChanged += (_, _) => Update();
        // The name is often known already: the password is what to type.
        dialog.Opened += (_, _) => (name.Text.Length > 0 ? (Control)password : name).Focus(FocusState.Programmatic);
        var result = await Show(dialog, opened);
        return result == ContentDialogResult.Primary ? new SignInCredentials(name.Text.Trim(), password.Password) : null;
    }

    /// <summary>
    /// Asks for a code from the user's authenticator app for
    /// <paramref name="gateway"/>, showing first the QR code of
    /// <paramref name="enrollment"/>, if given, that adds the gateway to the
    /// app; returns the code, or null if the user declined.
    /// <paramref name="opened"/> is told of the dialog, which the app hides
    /// if the stream ends.
    /// </summary>
    internal static async Task<string?> AskCode(
        XamlRoot root, string gateway, string? enrollment, bool retry, Action<ContentDialog?> opened)
    {
        var code = new TextBox
        {
            Header = "Code",
            PlaceholderText = "The code your authenticator app shows",
            MaxLength = MaxCodeLength,
            InputScope = new InputScope { Names = { new InputScopeName(InputScopeNameValue.Number) } },
        };
        var content = new StackPanel { Spacing = 12, MinWidth = 360 };
        var retried = retry ? "That code didn't work. " : "";
        if (enrollment is null)
        {
            content.Children.Add(Explanation($"{retried}Enter the code your authenticator app shows for {gateway}."));
        }
        else
        {
            content.Children.Add(Explanation(
                $"{retried}{gateway} signs you in with a code from an authenticator app, such as Microsoft "
                + "Authenticator, Google Authenticator or Authy. Scan this QR code with it, then enter the code it shows."));
            if (QrCodeImage.Create(enrollment) is { } image)
            {
                content.Children.Add(image);
            }
            content.Children.Add(CopyLinkButton(enrollment));
        }
        content.Children.Add(code);
        var dialog = new ContentDialog
        {
            XamlRoot = root,
            Title = enrollment is null ? $"Sign in to {gateway}" : "Add the gateway to your authenticator app",
            Content = content,
            PrimaryButtonText = "Sign in",
            CloseButtonText = "Cancel",
            DefaultButton = ContentDialogButton.Primary,
            IsPrimaryButtonEnabled = false,
        };
        code.TextChanged += (_, _) => dialog.IsPrimaryButtonEnabled = code.Text.Trim().Length > 0;
        dialog.Opened += (_, _) => code.Focus(FocusState.Programmatic);
        var result = await Show(dialog, opened);
        return result == ContentDialogResult.Primary ? code.Text.Trim() : null;
    }

    /// <summary>Shows <paramref name="dialog"/>, telling <paramref name="opened"/> of it while it shows.</summary>
    private static async Task<ContentDialogResult> Show(ContentDialog dialog, Action<ContentDialog?> opened)
    {
        opened(dialog);
        try
        {
            return await dialog.ShowAsync();
        }
        finally
        {
            opened(null);
        }
    }

    /// <summary>Returns a dialog's explanation of <paramref name="text"/>.</summary>
    private static TextBlock Explanation(string text) => new() { Text = text, TextWrapping = TextWrapping.Wrap };

    /// <summary>
    /// Returns a button that copies <paramref name="link"/>, for an
    /// authenticator app on this PC, or one that takes the address typed.
    /// </summary>
    private static HyperlinkButton CopyLinkButton(string link)
    {
        var button = new HyperlinkButton { Content = "Copy the link instead", Padding = new Thickness(0) };
        button.Click += (_, _) =>
        {
            var package = new DataPackage();
            package.SetText(link);
            Clipboard.SetContent(package);
            button.Content = "Copied";
        };
        return button;
    }
}

/// <summary>
/// A QR code drawn as the app shows it: dark modules on white, whatever the
/// theme, with its quiet zone, at whole pixels a module so that cameras
/// read it sharp.
/// </summary>
internal static class QrCodeImage
{
    /// <summary>The effective pixels a module takes.</summary>
    private const int ModulePixels = 4;

    /// <summary>The light modules around the code that readers need (ISO/IEC 18004).</summary>
    private const int QuietZone = 4;

    /// <summary>Returns the QR code of <paramref name="text"/>, or null if it is too long for one.</summary>
    internal static FrameworkElement? Create(string text)
    {
        if (Native.QrCodeOf(text) is not { } code)
        {
            return null;
        }
        var geometry = new GeometryGroup { FillRule = FillRule.Nonzero };
        for (var y = 0; y < code.Size; y++)
        {
            // A rectangle a run of dark modules, rather than one a module,
            // leaves no seams between them.
            var x = 0;
            while (x < code.Size)
            {
                if (!code.IsDark(x, y))
                {
                    x++;
                    continue;
                }
                var start = x;
                while (x < code.Size && code.IsDark(x, y))
                {
                    x++;
                }
                geometry.Children.Add(new RectangleGeometry
                {
                    Rect = new Rect(start * ModulePixels, y * ModulePixels, (x - start) * ModulePixels, ModulePixels),
                });
            }
        }
        var side = code.Size * ModulePixels;
        var image = new Border
        {
            Background = new SolidColorBrush(Colors.White),
            Padding = new Thickness(QuietZone * ModulePixels),
            HorizontalAlignment = HorizontalAlignment.Center,
            CornerRadius = new CornerRadius(4),
            Child = new Microsoft.UI.Xaml.Shapes.Path
            {
                Data = geometry,
                Fill = new SolidColorBrush(Colors.Black),
                Width = side,
                Height = side,
            },
        };
        AutomationProperties.SetName(image, "QR code that adds the gateway to an authenticator app");
        return image;
    }
}
