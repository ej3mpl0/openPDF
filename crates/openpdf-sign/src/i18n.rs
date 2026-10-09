//! User-facing strings for signing, via the shared gettext catalog in `po/`.
//!
//! Language comes from `LANG` / `LC_MESSAGES` (see [`openpdf_core::init_i18n`]);
//! English source strings are the fallback.

use openpdf_core::{
    tr, tr_args, CertificateInfo, CertificateUsage, CertificateValidity, SignatureInfo,
};

/// GNU gettext lookup (English `msgid` when the catalog has no translation).
pub fn gettext(msgid: &str) -> String {
    tr(msgid)
}

/// gettext with `{name}` placeholders (kept identical in `.po` msgstr).
pub fn gettext_f(msgid: &str, pairs: &[(&str, &str)]) -> String {
    tr_args(msgid, pairs)
}

/// User-facing PIN prompt for a token (translated).
pub fn pin_prompt_text(token_label: &str) -> String {
    tr_args("Enter PIN for token \"{token}\"", &[("token", token_label)])
}

/// Translated label for a certificate usage.
pub fn usage_label(usage: &CertificateUsage) -> String {
    match usage {
        CertificateUsage::Signing => tr("Signing"),
        CertificateUsage::Authentication => tr("Authentication"),
        CertificateUsage::Encryption => tr("Encryption"),
        CertificateUsage::Other(detail) => tr_args("Other ({detail})", &[("detail", detail)]),
    }
}

/// One-line certificate description for a picker (translated).
pub fn describe_certificate(info: &CertificateInfo) -> String {
    let usages = if info.usage.is_empty() {
        tr("unspecified usage")
    } else {
        info.usage
            .iter()
            .map(usage_label)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let validity = match (&info.validity.not_before, &info.validity.not_after) {
        (Some(a), Some(b)) => tr_args(
            "valid {not_before} to {not_after}",
            &[("not_before", a), ("not_after", b)],
        ),
        _ => tr("validity unknown"),
    };
    tr_args(
        "{subject} — {usage} — {validity}",
        &[
            ("subject", &info.subject),
            ("usage", &usages),
            ("validity", &validity),
        ],
    )
}

/// User-facing verify line for one signature dictionary (translated).
pub fn format_verify_result(info: &SignatureInfo) -> String {
    let mut parts = Vec::new();
    if let Some(name) = &info.name {
        parts.push(tr_args("Signer: {name}", &[("name", name)]));
    }
    if let Some(reason) = &info.reason {
        parts.push(tr_args("Reason: {reason}", &[("reason", reason)]));
    }
    if let Some(location) = &info.location {
        parts.push(tr_args("Location: {location}", &[("location", location)]));
    }
    parts.push(match info.valid {
        Some(true) => tr("Valid"),
        Some(false) => tr("Invalid"),
        None => tr("Validity unknown"),
    });
    if parts.len() == 1 && info.name.is_none() {
        parts.insert(0, tr("Signature"));
    }
    parts.join("; ")
}

/// Format every signature, or a “none found” line.
pub fn format_verify_results(infos: &[SignatureInfo]) -> String {
    if infos.is_empty() {
        return tr("No signatures found");
    }
    infos
        .iter()
        .map(format_verify_result)
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use openpdf_core::i18n::Catalog;
    use openpdf_core::set_locale;

    fn es_catalog() -> Catalog {
        Catalog::parse(include_str!("../../../po/es.po"))
    }

    #[test]
    fn english_is_source_string() {
        set_locale("en");
        assert_eq!(gettext("Signing"), "Signing");
        assert_eq!(
            pin_prompt_text("openpdf-test"),
            "Enter PIN for token \"openpdf-test\""
        );
    }

    #[test]
    fn spanish_from_lang() {
        set_locale("es_ES.UTF-8");
        let cat = es_catalog();
        assert_eq!(gettext("Signing"), cat.translate("Signing"));
        assert_eq!(gettext("Valid"), cat.translate("Valid"));
        assert_eq!(
            pin_prompt_text("DNIe"),
            cat.translate("Enter PIN for token \"{token}\"")
                .replace("{token}", "DNIe")
        );
        assert_eq!(
            gettext("certificate is not a signing cert"),
            cat.translate("certificate is not a signing cert")
        );
        set_locale("en");
    }

    #[test]
    fn c_locale_falls_back_to_english() {
        set_locale("C");
        assert_eq!(gettext("Invalid"), "Invalid");
        set_locale("en");
    }

    #[test]
    fn describe_certificate_follows_catalog() {
        let info = CertificateInfo {
            id: "1".into(),
            subject: "CN=Test".into(),
            issuer: "CN=CA".into(),
            validity: CertificateValidity {
                not_before: Some("2020-01-01T00:00:00Z".into()),
                not_after: Some("2030-01-01T00:00:00Z".into()),
            },
            usage: vec![CertificateUsage::Signing],
        };
        set_locale("en");
        assert_eq!(
            describe_certificate(&info),
            "CN=Test — Signing — valid 2020-01-01T00:00:00Z to 2030-01-01T00:00:00Z"
        );

        set_locale("es");
        let cat = es_catalog();
        let usage = cat.translate("Signing");
        let validity = cat
            .translate("valid {not_before} to {not_after}")
            .replace("{not_before}", "2020-01-01T00:00:00Z")
            .replace("{not_after}", "2030-01-01T00:00:00Z");
        let expected = cat
            .translate("{subject} — {usage} — {validity}")
            .replace("{subject}", "CN=Test")
            .replace("{usage}", usage)
            .replace("{validity}", &validity);
        assert_eq!(describe_certificate(&info), expected);
        assert_ne!(
            expected,
            "CN=Test — Signing — valid 2020-01-01T00:00:00Z to 2030-01-01T00:00:00Z"
        );
        set_locale("en");
    }

    #[test]
    fn verify_result_follows_catalog() {
        let info = SignatureInfo {
            name: Some("Ana".into()),
            reason: Some("agree".into()),
            location: Some("Madrid".into()),
            valid: Some(true),
        };
        set_locale("en");
        assert_eq!(
            format_verify_result(&info),
            "Signer: Ana; Reason: agree; Location: Madrid; Valid"
        );
        set_locale("es");
        let cat = es_catalog();
        let expected = [
            cat.translate("Signer: {name}").replace("{name}", "Ana"),
            cat.translate("Reason: {reason}")
                .replace("{reason}", "agree"),
            cat.translate("Location: {location}")
                .replace("{location}", "Madrid"),
            cat.translate("Valid").to_string(),
        ]
        .join("; ");
        assert_eq!(format_verify_result(&info), expected);
        assert_ne!(
            expected,
            "Signer: Ana; Reason: agree; Location: Madrid; Valid"
        );
        set_locale("en");
    }

    #[test]
    fn sign_error_msgid_is_translated_on_display() {
        use openpdf_core::Error;
        set_locale("es");
        let cat = es_catalog();
        let detail = cat.translate("not a PDF");
        let expected = cat.translate("sign: {detail}").replace("{detail}", detail);
        assert_eq!(Error::Sign("not a PDF".into()).to_string(), expected);
        assert_ne!(expected, "sign: not a PDF");
        set_locale("en");
    }
}
