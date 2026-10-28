//! Opaque references to immutable entries in the current user's system keychain.
//! A fresh entry keeps an existing saved configuration valid if saving its
//! replacement fails. No password or password-derived identifier is stored here.
use anyhow::{Result, ensure};

const PREFIX: &[u8] = b"WW-StudyApp/keychain/v1\0";

pub fn protect_with(input: &[u8], save: impl FnOnce(&str, &[u8]) -> Result<()>) -> Result<Vec<u8>> {
    let id = uuid::Uuid::new_v4().to_string();
    save(&id, input)?;
    let mut reference = PREFIX.to_vec();
    reference.extend_from_slice(id.as_bytes());
    Ok(reference)
}

pub fn unprotect_with(input: &[u8], load: impl FnOnce(&str) -> Result<Vec<u8>>) -> Result<Vec<u8>> {
    ensure!(
        input.starts_with(PREFIX) && input.len() == PREFIX.len() + 36,
        "凭据引用无效，请重新填写凭据"
    );
    let id = std::str::from_utf8(&input[PREFIX.len()..])?;
    ensure!(uuid::Uuid::parse_str(id)?.to_string() == id, "凭据引用无效");
    load(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn references_roundtrip_without_exposing_secrets_or_replacing_prior_values() {
        let mut store = BTreeMap::new();
        let mut protect = |secret: &[u8]| {
            protect_with(secret, |id, value| {
                assert!(store.insert(id.to_owned(), value.to_vec()).is_none());
                Ok(())
            })
            .unwrap()
        };
        let first = protect("私密密码".as_bytes());
        let second = protect(b"replacement");
        assert_ne!(first, second);
        assert!(!first.windows(6).any(|part| part == "私密".as_bytes()));
        let read = |id: &str| Ok(store.get(id).unwrap().clone());
        assert_eq!(unprotect_with(&first, read).unwrap(), "私密密码".as_bytes());
        assert_eq!(unprotect_with(&second, read).unwrap(), b"replacement");
    }

    #[test]
    fn malformed_references_never_access_the_keychain_and_store_failures_propagate() {
        for input in [b"foreign blob".as_slice(), PREFIX] {
            assert!(unprotect_with(input, |_| panic!("must not access keychain")).is_err());
        }
        let mut malformed = PREFIX.to_vec();
        malformed.extend_from_slice(b"../../../../../../../../../../../../");
        assert!(unprotect_with(&malformed, |_| panic!("must not access keychain")).is_err());
        assert!(protect_with(b"secret", |_, _| anyhow::bail!("locked")).is_err());
        let valid = protect_with(b"secret", |_, _| Ok(())).unwrap();
        assert!(unprotect_with(&valid, |_| anyhow::bail!("missing")).is_err());
    }
}
