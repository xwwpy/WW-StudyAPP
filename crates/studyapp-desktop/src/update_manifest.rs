//! Parsing and validation for the signed `latest.json` update manifest.
//!
//! The manifest is consumed by the NSIS updater introduced in the same change
//! series; keeping it separate makes the trust rules testable without network.
#![allow(dead_code)]
use base64::Engine;
use semver::Version;

const RELEASE_PREFIX: &str = "https://github.com/xwwpy/WW-StudyAPP/releases/download/";
pub fn current_platform() -> Result<&'static str, String> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Ok("windows-x86_64"),
        ("linux", "x86_64") => Ok("linux-x86_64"),
        ("macos", "aarch64") => Ok("macos-arm64"),
        ("macos", "x86_64") => Ok("macos-x86_64"),
        _ => Err("Unsupported update platform".into()),
    }
}

pub fn asset_name(version: &Version, platform: &str) -> Result<String, String> {
    let suffix = match platform {
        "windows-x86_64" => "-Setup.exe",
        "linux-x86_64" => ".deb",
        "macos-arm64" | "macos-x86_64" => ".dmg",
        _ => return Err("Unsupported update platform".into()),
    };
    Ok(format!("WW-StudyApp-{version}-{platform}{suffix}"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateManifest {
    pub version: Version,
    pub notes: String,
    pub pub_date: Option<String>,
    pub setup_url: String,
    pub signature: String,
    pub signed_envelope: String,
    pub size: u64,
    pub sha256: String,
}

impl UpdateManifest {
    pub fn is_newer_than(&self, current: &str) -> bool {
        Version::parse(current).is_ok_and(|value| self.version > value)
    }
}

pub fn parse_latest_json(text: &str) -> Result<UpdateManifest, String> {
    parse_with_key(
        text,
        current_platform()?,
        crate::update_signature::verification_key(),
    )
}

fn parse_with_key(
    text: &str,
    platform_name: &str,
    key: &[u8; 32],
) -> Result<UpdateManifest, String> {
    if text.len() > 2 * 1024 * 1024 {
        return Err("Manifest too large".into());
    }
    let signed_envelope = text.to_owned();
    let envelope: serde_json::Value = serde_json::from_str(text).map_err(|_| "Invalid JSON")?;
    let encoded = envelope["signed_payload"]
        .as_str()
        .ok_or("Unsigned manifest")?;
    let signature = envelope["manifest_signature"]
        .as_str()
        .ok_or("Unsigned manifest")?;
    let payload = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| "Invalid payload")?;
    let mut message = b"WW-StudyApp update manifest v1\0".to_vec();
    message.extend_from_slice(&payload);
    crate::update_signature::verify_with_key(&message, signature, key)?;
    let text = std::str::from_utf8(&payload).map_err(|_| "Invalid UTF-8")?;
    let root: serde_json::Value =
        serde_json::from_str(text).map_err(|_| "更新清单不是有效的 JSON".to_owned())?;
    let version_text = root
        .get("version")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "更新清单缺少版本号".to_owned())?;
    let version = Version::parse(version_text).map_err(|_| "更新清单版本号无效".to_owned())?;
    let notes = root
        .get("notes")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_owned();
    let pub_date = root
        .get("pub_date")
        .and_then(|value| value.as_str())
        .map(str::to_owned);
    let platform = root
        .get("platforms")
        .and_then(|value| value.get(platform_name))
        .ok_or_else(|| "更新清单缺少 windows-x86_64 条目".to_owned())?;
    let signature = platform
        .get("signature")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "更新清单缺少签名".to_owned())?
        .to_owned();
    let setup_url = platform
        .get("url")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "更新清单缺少下载地址".to_owned())?;
    if !version.pre.is_empty() || !version.build.is_empty() || root["schema"] != 1 {
        return Err("Unsupported release version or schema".into());
    }
    let expected = format!(
        "{RELEASE_PREFIX}v{version}/{}",
        asset_name(&version, platform_name)?
    );
    if setup_url != expected {
        return Err("Unexpected package URL".into());
    }
    let size = platform["size"].as_u64().ok_or("Missing package size")?;
    let hash = platform["sha256"]
        .as_str()
        .ok_or("Missing package digest")?;
    if size == 0
        || size > 256 * 1024 * 1024
        || hash.len() != 64
        || !hash.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("Invalid package size or digest".into());
    }
    Ok(UpdateManifest {
        version,
        notes,
        pub_date,
        setup_url: setup_url.to_owned(),
        signature,
        signed_envelope,
        size,
        sha256: hash.to_ascii_lowercase(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"{
      "schema": 1, "version": "0.9.0",
      "notes": "- 新版本",
      "pub_date": "2026-10-28T00:00:00Z",
      "platforms": {
        "windows-x86_64": {
          "size": 10, "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "signature": "c2lnbmF0dXJl",
          "url": "https://github.com/xwwpy/WW-StudyAPP/releases/download/v0.9.0/WW-StudyApp-0.9.0-windows-x86_64-Setup.exe"
        }
      }
    }"#;

    fn signed_parse(body: &str, platform: &str) -> Result<UpdateManifest, String> {
        use ed25519_dalek::{Signer, SigningKey};
        let key = SigningKey::from_bytes(&[17; 32]);
        let mut message = b"WW-StudyApp update manifest v1\0".to_vec();
        message.extend_from_slice(body.as_bytes());
        let envelope = serde_json::json!({
            "signed_payload": base64::engine::general_purpose::STANDARD.encode(body),
            "manifest_signature": base64::engine::general_purpose::STANDARD.encode(key.sign(&message).to_bytes())
        });
        parse_with_key(
            &envelope.to_string(),
            platform,
            &key.verifying_key().to_bytes(),
        )
    }

    #[::core::prelude::v1::test]
    fn all_platforms_select_exact_asset_and_reject_wrong_tag() {
        for platform in [
            "windows-x86_64",
            "linux-x86_64",
            "macos-arm64",
            "macos-x86_64",
        ] {
            let name = asset_name(&Version::parse("0.9.0").unwrap(), platform).unwrap();
            let body = VALID
                .replace("WW-StudyApp-0.9.0-windows-x86_64-Setup.exe", &name)
                .replace("windows-x86_64", platform);
            assert!(signed_parse(&body, platform).is_ok());
            assert!(signed_parse(&body.replace("/v0.9.0/", "/v0.8.0/"), platform).is_err());
        }
    }

    #[::core::prelude::v1::test]
    fn rejects_unsigned_metadata_even_when_package_has_signature() {
        assert!(parse_latest_json(VALID).is_err());
    }

    #[::core::prelude::v1::test]
    fn authenticated_package_constraints_are_retained() {
        let manifest = signed_parse(VALID, "windows-x86_64").unwrap();
        assert_eq!(manifest.size, 10);
        assert_eq!(manifest.sha256, "a".repeat(64));
    }

    #[::core::prelude::v1::test]
    fn valid_manifest_parses_and_compares_versions() {
        let manifest = signed_parse(VALID, "windows-x86_64").unwrap();
        assert_eq!(manifest.version.to_string(), "0.9.0");
        assert_eq!(manifest.notes, "- 新版本");
        assert!(manifest.is_newer_than("0.8.5"));
        assert!(!manifest.is_newer_than("0.9.0"));
        assert!(!manifest.is_newer_than("0.9.1"));
        assert!(!manifest.is_newer_than("not-a-version"));
    }

    #[::core::prelude::v1::test]
    fn manifest_rejects_missing_fields_and_untrusted_urls() {
        assert!(parse_latest_json("not json").is_err());
        assert!(parse_latest_json(r#"{"platforms":{}}"#).is_err());
        assert!(parse_latest_json(r#"{"version":"x","platforms":{}}"#).is_err());
        assert!(parse_latest_json(r#"{"version":"1.0.0"}"#).is_err());
        for url in [
            "http://github.com/xwwpy/WW-StudyAPP/releases/download/v1/WW-StudyApp-1.0.0-windows-x86_64-Setup.exe",
            "https://example.com/x/WW-StudyApp-1.0.0-windows-x86_64-Setup.exe",
            "https://github.com/xwwpy/WW-StudyAPP/releases/download/v1/WW-StudyApp-1.0.0-windows-x86_64.zip",
            "https://github.com/xwwpy/WW-StudyAPP/releases/download/v1/evil.exe",
        ] {
            let text = VALID.replace(
                "https://github.com/xwwpy/WW-StudyAPP/releases/download/v0.9.0/WW-StudyApp-0.9.0-windows-x86_64-Setup.exe",
                url,
            );
            assert!(signed_parse(&text, "windows-x86_64").is_err(), "{url}");
        }
        let empty_signature = VALID.replace("c2lnbmF0dXJl", "");
        assert!(parse_latest_json(&empty_signature).is_err());
    }
}
