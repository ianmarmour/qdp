import SwiftUI

/// Asks for the four-digit code the host shows, answering as soon as the
/// last digit is typed.
struct PairingView: View {
    /// The host's name.
    var hostName: String
    /// Whether the last code did not work.
    var retry: Bool
    /// Answers with the code typed, or `nil` if the user cancelled.
    var answer: (String?) -> Void

    @State private var code = ""
    @FocusState private var focused: Bool

    /// The digits of a code.
    private static let length = 4

    var body: some View {
        VStack(spacing: 20) {
            Image(systemName: "lock.desktopcomputer")
                .font(.system(size: 40))
                .foregroundStyle(.tint)
            VStack(spacing: 6) {
                Text("Pair with \(hostName)")
                    .font(.title2.bold())
                Text(retry
                    ? "That code didn't match. \(hostName) is showing a new one."
                    : "Enter the 4-digit code shown on \(hostName).")
                    .multilineTextAlignment(.center)
                    .foregroundStyle(retry ? Color.orange : Color.secondary)
            }
            ZStack {
                HStack(spacing: 12) {
                    ForEach(0..<Self.length, id: \.self) { index in
                        cell(index)
                    }
                }
                TextField("", text: $code)
                    .focused($focused)
                    .textContentType(.oneTimeCode)
                    #if os(iOS)
                    .keyboardType(.numberPad)
                    #endif
                    .opacity(0.02)
                    .accessibilityLabel("Pairing code")
                    .onChange(of: code) { _, typed in
                        let digits = String(typed.filter(\.isNumber).prefix(Self.length))
                        if digits != typed {
                            code = digits
                        }
                        if digits.count == Self.length {
                            answer(digits)
                        }
                    }
            }
            .frame(height: 64)
            .onTapGesture { focused = true }
            Button("Cancel", role: .cancel) { answer(nil) }
                .keyboardShortcut(.cancelAction)
        }
        .padding(28)
        .frame(minWidth: 320)
        .onAppear { focused = true }
        .onChange(of: retry) { _, _ in code = "" }
    }

    private func cell(_ index: Int) -> some View {
        let characters = Array(code)
        let digit = index < characters.count ? String(characters[index]) : ""
        let active = index == characters.count
        return Text(digit)
            .font(.system(size: 32, weight: .semibold, design: .rounded).monospacedDigit())
            .frame(width: 52, height: 64)
            .background(.quaternary, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .strokeBorder(active && focused ? Color.accentColor : .clear, lineWidth: 2)
            }
    }
}
