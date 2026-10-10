// SPDX-License-Identifier: MPL-2.0
import Foundation

/// Chrome strings go through core gettext (`translate`) so `po/es.po` is
/// the only Spanish source. Bundle `.strings` generated from the same po
/// are a fallback before the cdylib is loaded.
func L(_ msgid: String) -> String {
    let fromCore = translate(msgid: msgid)
    if fromCore != msgid {
        return fromCore
    }
    return String(localized: String.LocalizationValue(msgid))
}

func L(_ msgid: String, _ args: [String: String]) -> String {
    var out = L(msgid)
    for (key, value) in args {
        out = out.replacingOccurrences(of: "{\(key)}", with: value)
    }
    return out
}
