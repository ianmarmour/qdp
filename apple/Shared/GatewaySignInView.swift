import CoreImage
import CoreImage.CIFilterBuiltins
import SwiftUI
#if os(macOS)
import AppKit
#else
import UIKit
#endif

/// What the gateway asks while the user signs in: their name and password,
/// or, with `code`, a code from their authenticator app, which `enrollment`,
/// an `otpauth://` URI, adds to the app first if given; `retry` after an
/// answer that did not work.
struct GatewayPrompt: Equatable {
    var gateway: String
    var retry: Bool
    var code: Bool
    var enrollment: String?
}

/// Signs in to the gateway: the user's name and password, then a code from
/// their authenticator app. The first time, it offers to add the gateway to
/// an authenticator app on this device and shows the QR code that adds it
/// to one on another.
struct GatewaySignInView: View {
    var prompt: GatewayPrompt
    /// Answers the name and password, or declines with `nil`.
    var signIn: ((user: String, password: String)?) -> Void
    /// Answers the code, or declines with `nil`.
    var enterCode: (String?) -> Void

    @Environment(\.openURL) private var openURL
    @State private var user = Gateway.user
    @State private var password = ""
    @State private var code = ""

    /// The digits of a gateway's code (Section 4.4.10).
    private static let codeDigits = 6

    var body: some View {
        NavigationStack {
            Form {
                if prompt.code {
                    codeSection
                } else {
                    credentialsSection
                }
            }
            .formStyle(.grouped)
            .navigationTitle("Sign In to \(prompt.gateway)")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { prompt.code ? enterCode(nil) : signIn(nil) }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Sign In") { submit() }
                        .disabled(!ready)
                }
            }
        }
        #if os(macOS)
        .frame(minWidth: 420, minHeight: prompt.enrollment == nil ? 240 : 520)
        #endif
    }

    @ViewBuilder private var credentialsSection: some View {
        Section {
            TextField("User name", text: $user)
                .textContentType(.username)
                .autocorrectionDisabled()
                #if os(iOS)
                .textInputAutocapitalization(.never)
                #endif
            SecureField("Password", text: $password)
                .textContentType(.password)
                .onSubmit(submit)
        } footer: {
            Text(prompt.retry ? "That didn't work. Check your user name and password." : "Use your organization's account.")
        }
    }

    @ViewBuilder private var codeSection: some View {
        if let enrollment = prompt.enrollment {
            Section {
                if let url = URL(string: enrollment) {
                    Button("Add to Authenticator App") { openURL(url) }
                }
                if let image = Self.qrCode(enrollment) {
                    HStack {
                        Spacer()
                        Image(decorative: image, scale: 1)
                            .interpolation(.none)
                            .resizable()
                            .frame(width: 200, height: 200)
                            .accessibilityLabel("QR code that adds the gateway to an authenticator app")
                        Spacer()
                    }
                }
                Button("Copy Setup Link") { Self.copy(enrollment) }
            } footer: {
                Text("Add the gateway to an authenticator app, such as Google Authenticator, Microsoft Authenticator or Authy: on this device, or by scanning the QR code with another.")
            }
        }
        Section {
            TextField("Code", text: $code)
                .textContentType(.oneTimeCode)
                .font(.title3.monospacedDigit())
                #if os(iOS)
                .keyboardType(.numberPad)
                #endif
                .onChange(of: code) { _, typed in
                    let digits = String(typed.filter(\.isNumber).prefix(Self.codeDigits))
                    if digits != typed { code = digits }
                }
                .onSubmit(submit)
        } footer: {
            Text(prompt.retry ? "That code didn't work. Enter the code your authenticator app shows now." : "Enter the six-digit code your authenticator app shows.")
        }
    }

    private var ready: Bool {
        prompt.code ? code.count == Self.codeDigits : !user.trimmingCharacters(in: .whitespaces).isEmpty && !password.isEmpty
    }

    private func submit() {
        guard ready else { return }
        if prompt.code {
            enterCode(code)
        } else {
            signIn((user.trimmingCharacters(in: .whitespaces), password))
        }
    }

    /// Returns the QR code of `text`, a module a pixel with its quiet zone,
    /// which the view scales up without smoothing.
    private static func qrCode(_ text: String) -> CGImage? {
        let filter = CIFilter.qrCodeGenerator()
        filter.message = Data(text.utf8)
        filter.correctionLevel = "M"
        guard let image = filter.outputImage else { return nil }
        return CIContext().createCGImage(image, from: image.extent)
    }

    /// Copies the setup link, for an authenticator that takes one pasted.
    private static func copy(_ text: String) {
        #if os(macOS)
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
        #else
        UIPasteboard.general.string = text
        #endif
    }
}
