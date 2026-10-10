// SPDX-License-Identifier: MPL-2.0
import AppKit
import Foundation

/// UniFFI `PinProvider`. Empty field or Cancel → `FfiError.PinCancelled`
/// (never an empty `SecretPin`). Shows `pin_prompt` plus `describe_pin_status`.
final class MacPinProvider: NSObject, PinProvider, @unchecked Sendable {
    /// PKCS#11 flags from the picker when FFI grows `Certificate.pin_status`.
    var countLow = false
    var finalTry = false
    var locked = false

    func requestPin(tokenLabel: String) throws -> SecretPin {
        let status = describePinStatus(
            countLow: countLow,
            finalTry: finalTry,
            locked: locked
        )
        let prompt = pinPrompt(tokenLabel: tokenLabel)
        if Thread.isMainThread {
            return try Self.present(prompt: prompt, status: status)
        }
        return try DispatchQueue.main.sync {
            try Self.present(prompt: prompt, status: status)
        }
    }

    static func present(prompt: String, status: String?) throws -> SecretPin {
        let alert = NSAlert()
        alert.messageText = L("Enter PIN")
        var info = prompt
        if let status, !status.isEmpty {
            info += "\n\n" + status
        }
        alert.informativeText = info
        alert.addButton(withTitle: L("Unlock"))
        alert.addButton(withTitle: L("Cancel"))
        let field = NSSecureTextField(frame: NSRect(x: 0, y: 0, width: 260, height: 24))
        alert.accessoryView = field
        alert.window.initialFirstResponder = field
        let response = alert.runModal()
        let value = field.stringValue
        field.stringValue = ""
        if response != .alertFirstButtonReturn || value.isEmpty {
            throw FfiError.PinCancelled(message: L("PIN entry was cancelled"))
        }
        return SecretPin(value: value)
    }
}

/// `PinCancelled` is a silent cancel — no red error banner.
func isSilentPinCancel(_ error: Error) -> Bool {
    if case FfiError.PinCancelled = error {
        return true
    }
    return false
}
