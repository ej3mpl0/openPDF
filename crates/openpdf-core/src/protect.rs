use std::collections::BTreeMap;
use std::sync::Arc;

use lopdf::encryption::crypt_filters::{Aes256CryptFilter, CryptFilter};
use lopdf::{EncryptionState, EncryptionVersion, Object, Permissions, StringFormat};
use zeroize::Zeroizing;

use crate::document::Document;
use crate::error::{Error, InvalidKind, Result, SignedOp};

fn fill_random(buf: &mut [u8]) -> Result<()> {
    getrandom::fill(buf).map_err(|e| Error::structure(e.to_string()))
}

fn random_aes_key() -> Result<Zeroizing<[u8; 32]>> {
    let mut key = Zeroizing::new([0u8; 32]);
    fill_random(key.as_mut())?;
    Ok(key)
}

fn random_file_id() -> Result<Zeroizing<[u8; 16]>> {
    let mut id = Zeroizing::new([0u8; 16]);
    fill_random(id.as_mut())?;
    Ok(id)
}

fn ensure_file_ids(doc: &mut lopdf::Document) -> Result<()> {
    if doc.trailer.get(b"ID").is_ok() {
        return Ok(());
    }
    let first = random_file_id()?;
    let second = random_file_id()?;
    doc.trailer.set(
        "ID",
        vec![
            Object::String(first.to_vec(), StringFormat::Hexadecimal),
            Object::String(second.to_vec(), StringFormat::Hexadecimal),
        ],
    );
    Ok(())
}

/// User-facing permission flags (AES-256 / revision 6).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PdfPermissions {
    pub print: bool,
    pub modify: bool,
    pub copy: bool,
    pub annotate: bool,
    pub fill: bool,
    pub assemble: bool,
    pub print_high: bool,
}

impl PdfPermissions {
    pub fn all() -> Self {
        Self {
            print: true,
            modify: true,
            copy: true,
            annotate: true,
            fill: true,
            assemble: true,
            print_high: true,
        }
    }

    pub fn parse_list(spec: &str) -> Result<Self> {
        if spec.trim().is_empty() || spec.trim() == "all" {
            return Ok(Self::all());
        }
        let mut p = Self::default();
        p.copy = true;
        for part in spec.split(',') {
            match part.trim() {
                "" => {}
                "print" => p.print = true,
                "modify" => p.modify = true,
                "copy" => p.copy = true,
                "annotate" => p.annotate = true,
                "fill" => p.fill = true,
                "assemble" => p.assemble = true,
                "print-high" | "printhigh" => p.print_high = true,
                other => {
                    return Err(Error::invalid(InvalidKind::UnknownPermission {
                        value: other.to_string(),
                    }));
                }
            }
        }
        Ok(p)
    }

    fn to_lopdf(self) -> Permissions {
        let mut bits = Permissions::COPYABLE_FOR_ACCESSIBILITY;
        if self.print {
            bits |= Permissions::PRINTABLE;
        }
        if self.modify {
            bits |= Permissions::MODIFIABLE;
        }
        if self.copy {
            bits |= Permissions::COPYABLE;
        }
        if self.annotate {
            bits |= Permissions::ANNOTABLE;
        }
        if self.fill {
            bits |= Permissions::FILLABLE;
        }
        if self.assemble {
            bits |= Permissions::ASSEMBLABLE;
        }
        if self.print_high {
            bits |= Permissions::PRINTABLE_IN_HIGH_QUALITY;
        }
        bits
    }
}

/// AES-256 (V5 / R6) passwords and permissions.
///
/// Passwords live in [`Zeroizing`] so they are wiped when this struct is dropped
/// (same contract as `SecretPin` on the signing side).
#[derive(Clone, Debug)]
pub struct ProtectOptions {
    pub user_password: Zeroizing<String>,
    pub owner_password: Zeroizing<String>,
    pub permissions: PdfPermissions,
    pub encrypt_metadata: bool,
}

impl ProtectOptions {
    pub fn new(user: impl Into<String>, owner: impl Into<String>) -> Result<Self> {
        let user_password = Zeroizing::new(user.into());
        let mut owner_password = Zeroizing::new(owner.into());
        if user_password.is_empty() && owner_password.is_empty() {
            return Err(Error::invalid(InvalidKind::PasswordRequired));
        }
        if owner_password.is_empty() {
            *owner_password = (*user_password).clone();
        }
        Ok(Self {
            user_password,
            owner_password,
            permissions: PdfPermissions::all(),
            encrypt_metadata: true,
        })
    }
}

/// Encrypt with AES-256. Signed documents are refused.
pub fn protect(doc: &Document, opts: &ProtectOptions) -> Result<Document> {
    if doc.is_signed() {
        return Err(Error::signed(SignedOp::Rewrite));
    }
    if doc.is_encrypted() {
        return Err(Error::invalid(InvalidKind::AlreadyEncrypted));
    }
    let mut inner = doc.assembled_lopdf()?;
    ensure_file_ids(&mut inner)?;
    let file_key = random_aes_key()?;
    let crypt_filter: Arc<dyn CryptFilter> = Arc::new(Aes256CryptFilter);
    let version = EncryptionVersion::V5 {
        encrypt_metadata: opts.encrypt_metadata,
        crypt_filters: BTreeMap::from([(b"StdCF".to_vec(), crypt_filter)]),
        file_encryption_key: file_key.as_ref(),
        stream_filter: b"StdCF".to_vec(),
        string_filter: b"StdCF".to_vec(),
        owner_password: &opts.owner_password,
        user_password: &opts.user_password,
        permissions: opts.permissions.to_lopdf(),
    };
    let state = EncryptionState::try_from(version).map_err(|e| Error::structure(e.to_string()))?;
    inner
        .encrypt(&state)
        .map_err(|e| Error::structure(e.to_string()))?;
    drop(state);
    Document::from_lopdf(inner, None)
}

/// Decrypt with the user or owner password.
pub fn unprotect(doc: &Document, password: &str) -> Result<Document> {
    if doc.is_signed() {
        return Err(Error::signed(SignedOp::Rewrite));
    }
    if !doc.is_encrypted() {
        return Err(Error::invalid(InvalidKind::NotEncrypted));
    }
    if password.is_empty() {
        return Err(Error::invalid(InvalidKind::PasswordRequired));
    }
    let password = Zeroizing::new(password.to_string());
    let mut inner = doc.assembled_lopdf()?;
    inner
        .decrypt(&password)
        .map_err(|_| Error::invalid(InvalidKind::WrongPassword))?;
    inner.trailer.remove(b"Encrypt");
    Document::from_lopdf(inner, None)
}

/// Drop identifying metadata, JavaScript, and embedded-file name trees.
pub fn sanitize_metadata(doc: &Document) -> Result<Document> {
    if doc.is_signed() {
        return Err(Error::signed(SignedOp::Edit));
    }
    let mut inner = doc.assembled_lopdf()?;
    if let Ok(Object::Reference(id)) = inner.trailer.get(b"Info").map(|o| o.to_owned()) {
        inner.objects.remove(&id);
    }
    inner.trailer.remove(b"Info");
    if let Ok(catalog) = inner.catalog_mut() {
        for key in [
            b"Metadata".as_slice(),
            b"OpenAction",
            b"AA",
            b"JS",
            b"JavaScript",
            b"Names",
            b"PieceInfo",
            b"Perms",
        ] {
            catalog.remove(key);
        }
    }
    Document::from_lopdf(inner, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signer::append_stub_signature;
    use crate::test_support::corpus;

    #[test]
    fn random_key_and_id_are_nonzero() {
        let key = random_aes_key().unwrap();
        assert!(key.iter().any(|b| *b != 0));
        let id = random_file_id().unwrap();
        assert!(id.iter().any(|b| *b != 0));
    }

    #[test]
    fn protect_unprotect_roundtrip() {
        let doc = Document::open(corpus("hello.pdf")).unwrap();
        let opts = ProtectOptions::new("user-secret", "owner-secret").unwrap();
        let locked = protect(&doc, &opts).unwrap();
        assert!(locked.is_encrypted());
        assert_eq!(locked.page_count(), 1);
        let open = unprotect(&locked, "user-secret").unwrap();
        assert!(!open.is_encrypted());
        assert_eq!(open.page_count(), 1);
    }

    #[test]
    fn sanitize_drops_info() {
        let doc = Document::open(corpus("hello.pdf")).unwrap();
        let clean = sanitize_metadata(&doc).unwrap();
        assert!(!clean.assembled_lopdf().unwrap().trailer.has(b"Info"));
    }

    #[test]
    fn protect_refuses_signed() {
        let bytes = append_stub_signature(&std::fs::read(corpus("hello.pdf")).unwrap()).unwrap();
        let signed = Document::from_bytes(bytes, None).unwrap();
        let opts = ProtectOptions::new("x", "y").unwrap();
        assert!(matches!(
            protect(&signed, &opts),
            Err(Error::Signed {
                op: SignedOp::Rewrite
            })
        ));
    }
}
