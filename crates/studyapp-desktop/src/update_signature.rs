//! Ed25519 verification for the NSIS setup advertised by `latest.json`.
//!
//! The updater downloads `latest.json`, then the setup it points at, and runs
//! the installer only when the signature over the exact setup bytes validates
//! against the release key compiled into the application.
#![allow(dead_code)] // consumed by the NSIS updater in the same change series.
use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};

/// Release signing key. The matching private key exists only as a CI secret and
/// never enters the repository; rotate both together if it is ever exposed.
pub const UPDATE_SIGNING_KEY: [u8; 32] = [
    0xe4, 0x01, 0x9e, 0x1a, 0x94, 0x6e, 0xa1, 0x6f, 0xbc, 0x82, 0x89, 0x77, 0xbf, 0x61, 0xa6, 0xac,
    0x33, 0x04, 0x51, 0x91, 0x84, 0x1f, 0x3d, 0x73, 0x72, 0x7e, 0x41, 0x48, 0xec, 0x07, 0x09, 0x4b,
];

/// Verifies a manifest signature against the compiled-in release key.
pub fn verify_manifest_signature(payload: &[u8], signature_base64: &str) -> Result<(), String> {
    verify_with_key(payload, signature_base64, &UPDATE_SIGNING_KEY)
}

/// Verifies a signature against an explicit key; used by tests and rotation.
pub fn verify_with_key(
    payload: &[u8],
    signature_base64: &str,
    key: &[u8; 32],
) -> Result<(), String> {
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(signature_base64.trim())
        .map_err(|_| "更新签名不是有效的 Base64".to_owned())?;
    let signature = Signature::from_slice(&decoded).map_err(|_| "更新签名长度不正确".to_owned())?;
    let verifying = VerifyingKey::from_bytes(key).map_err(|_| "更新公钥无效".to_owned())?;
    verifying
        .verify(payload, &signature)
        .map_err(|_| "更新包签名校验失败".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 8032 test vector 1: empty message.
    const VECTOR_KEY: [u8; 32] = [
        0xd7, 0x5a, 0x98, 0x01, 0x82, 0xb1, 0x0a, 0xb7, 0xd5, 0x4b, 0xfe, 0xd3, 0xc9, 0x64, 0x07,
        0x3a, 0x0e, 0xe1, 0x72, 0xf3, 0xda, 0xa6, 0x23, 0x25, 0xaf, 0x02, 0x1a, 0x68, 0xf7, 0x07,
        0x51, 0x1a,
    ];
    const VECTOR_SIGNATURE: &str =
        "5VZDAMNgrHKQhuLMgG6CioSHfx645dl02HPgZSJJAVVfuIIVkKM7rMYeOXAc+bRr0lv18FlbviRlUUFDjnoQCw==";

    #[::core::prelude::v1::test]
    fn rfc8032_vector_verifies_and_tampering_fails() {
        assert!(verify_with_key(b"", VECTOR_SIGNATURE, &VECTOR_KEY).is_ok());
        assert!(verify_with_key(b"x", VECTOR_SIGNATURE, &VECTOR_KEY).is_err());
        let mut corrupted = base64::engine::general_purpose::STANDARD
            .decode(VECTOR_SIGNATURE)
            .unwrap();
        corrupted[0] ^= 1;
        let corrupted = base64::engine::general_purpose::STANDARD.encode(&corrupted);
        assert!(verify_with_key(b"", &corrupted, &VECTOR_KEY).is_err());
    }

    #[::core::prelude::v1::test]
    fn malformed_forgery_attempts_are_rejected() {
        assert!(verify_with_key(b"payload", "not base64!", &VECTOR_KEY).is_err());
        assert!(verify_with_key(b"payload", "AAAA", &VECTOR_KEY).is_err());
        let mut wrong_key = VECTOR_KEY;
        wrong_key[0] ^= 1;
        assert!(verify_with_key(b"", VECTOR_SIGNATURE, &wrong_key).is_err());
    }

    #[::core::prelude::v1::test]
    fn release_key_is_well_formed() {
        assert!(VerifyingKey::from_bytes(&UPDATE_SIGNING_KEY).is_ok());
        // The release key must not be the RFC vector key.
        assert_ne!(UPDATE_SIGNING_KEY, VECTOR_KEY);
    }
}
