// SPDX-License-Identifier: MPL-2.0
/// Shared gettext catalog for Swift / C# chrome (PIN dialog, banners).
///
/// Foreign UIs should call these instead of shipping a second catalog.
/// [`describe_pin_status`] uses the same English msgids as core on the
/// sign-native stack so `po/es.po` stays the single Spanish source.
use crate::ensure_i18n;

/// Translate an English `msgid` through the embedded catalog.
#[uniffi::export]
pub fn translate(msgid: String) -> String {
    ensure_i18n();
    openpdf_core::tr(&msgid)
}

/// Localized “Enter PIN for token …” line.
#[uniffi::export]
pub fn pin_prompt(token_label: String) -> String {
    ensure_i18n();
    openpdf_core::pin_prompt_text(&token_label)
}

/// Translated PIN-retry warning, or `None` when there is nothing to show.
///
/// Mirrors `openpdf_core::describe_pin_status` (PR #11). The FFI `Certificate`
/// record does not yet carry `pin_status`; the UI passes the flags it has
/// (all-false → `None`).
#[uniffi::export]
pub fn describe_pin_status(count_low: bool, final_try: bool, locked: bool) -> Option<String> {
    ensure_i18n();
    if locked {
        Some(openpdf_core::tr("Token PIN is locked"))
    } else if final_try {
        Some(openpdf_core::tr("Last PIN attempt"))
    } else if count_low {
        Some(openpdf_core::tr("Few PIN attempts remaining"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_pin_status_priority() {
        openpdf_core::set_locale("en");
        assert_eq!(describe_pin_status(false, false, false), None);
        assert_eq!(
            describe_pin_status(true, false, false).as_deref(),
            Some("Few PIN attempts remaining")
        );
        assert_eq!(
            describe_pin_status(true, true, false).as_deref(),
            Some("Last PIN attempt")
        );
        assert_eq!(
            describe_pin_status(true, true, true).as_deref(),
            Some("Token PIN is locked")
        );
    }

    #[test]
    fn translate_falls_back_to_msgid() {
        openpdf_core::set_locale("en");
        assert_eq!(
            translate("PIN entry was cancelled".into()),
            "PIN entry was cancelled"
        );
        assert_eq!(
            pin_prompt("DNIe".into()),
            openpdf_core::pin_prompt_text("DNIe")
        );
    }
}
