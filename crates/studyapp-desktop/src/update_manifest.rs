//! Parsing and validation for the signed `latest.json` update manifest.
//!
//! The manifest is consumed by the NSIS updater introduced in the same change
//! series; keeping it separate makes the trust rules testable without network.
#![allow(dead_code)]
use semver::Version;

const RELEASE_PREFIX: &str = "https://github.com/xwwpy/WW-StudyAPP/releases/download/";
const PLATFORM: &str = "windows-x86_64";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateManifest {
    pub version: Version,
    pub notes: String,
    pub pub_date: Option<String>,
    pub setup_url: String,
    pub signature: String,
}

impl UpdateManifest {
    pub fn is_newer_than(&self, current: &str) -> bool {
        Version::parse(current).is_ok_and(|value| self.version > value)
    }
}

pub fn parse_latest_json(text: &str) -> Result<UpdateManifest, String> {
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
        .and_then(|value| value.get(PLATFORM))
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
    if !setup_url.starts_with(RELEASE_PREFIX) {
        return Err("更新清单下载地址不是本仓库的发布地址".into());
    }
    let file_name = setup_url.rsplit('/').next().unwrap_or_default();
    if !file_name.ends_with("-Setup.exe") {
        return Err("更新清单下载地址不是 NSIS 安装包".into());
    }
    Ok(UpdateManifest {
        version,
        notes,
        pub_date,
        setup_url: setup_url.to_owned(),
        signature,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"{
      "version": "0.9.0",
      "notes": "- 新版本",
      "pub_date": "2026-10-28T00:00:00Z",
      "platforms": {
        "windows-x86_64": {
          "signature": "c2lnbmF0dXJl",
          "url": "https://github.com/xwwpy/WW-StudyAPP/releases/download/v0.9.0/WW-StudyApp-0.9.0-windows-x86_64-Setup.exe"
        }
      }
    }"#;

    #[::core::prelude::v1::test]
    fn valid_manifest_parses_and_compares_versions() {
        let manifest = parse_latest_json(VALID).unwrap();
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
            assert!(parse_latest_json(&text).is_err(), "{url}");
        }
        let empty_signature = VALID.replace("c2lnbmF0dXJl", "");
        assert!(parse_latest_json(&empty_signature).is_err());
    }
}
