use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImageStorage {
    #[default]
    Local,
    Cloud,
    Both,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ImageSettings {
    pub root: Option<String>,
    pub storage: ImageStorage,
    pub cloud: ImageCloudTarget,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum ImageCloudTarget {
    /// Explicit compatibility choice for images configured before independent targets.
    #[default]
    NoteSync,
    ObjectStore(ObjectStoreSettings),
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ObjectAddressing {
    #[default]
    VirtualHosted,
    Path,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ObjectStoreSettings {
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key_id: String,
    pub prefix: String,
    pub addressing: ObjectAddressing,
}

impl ObjectStoreSettings {
    pub fn validate(&self) -> Result<()> {
        let authority = self
            .endpoint
            .strip_prefix("https://")
            .unwrap_or("")
            .trim_end_matches('/');
        ensure!(
            !authority.is_empty()
                && self.endpoint.len() <= 4096
                && !authority.chars().any(|c| c.is_control()
                    || c.is_whitespace()
                    || matches!(c, '/' | '@' | '?' | '#')),
            "对象存储 Endpoint 须为不含账户、查询参数或路径的 HTTPS 地址"
        );
        ensure!(
            (3..=63).contains(&self.bucket.len())
                && self.bucket.bytes().all(|b| b.is_ascii_lowercase()
                    || b.is_ascii_digit()
                    || matches!(b, b'-' | b'.'))
                && self.bucket.starts_with(|c: char| c.is_ascii_alphanumeric())
                && self.bucket.ends_with(|c: char| c.is_ascii_alphanumeric()),
            "请填写有效的 Bucket 名称"
        );
        ensure!(
            !self.region.is_empty()
                && self.region.len() <= 128
                && self
                    .region
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')),
            "请填写服务商要求的 Region"
        );
        ensure!(
            !self.access_key_id.is_empty()
                && self.access_key_id.len() <= 256
                && self
                    .access_key_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'@')),
            "请填写有效的 Access Key ID"
        );
        ensure!(
            self.prefix.len() <= 512
                && !self.prefix.chars().any(char::is_control)
                && !self.prefix.contains(['\\', '%', '?', '#'])
                && (self.prefix.is_empty()
                    || self
                        .prefix
                        .split('/')
                        .all(|part| !matches!(part, "" | "." | ".."))),
            "对象前缀不能包含首尾斜杠、空段或上级跳转"
        );
        Ok(())
    }
}

pub fn validate_images(settings: &ImageSettings) -> Result<()> {
    if settings.storage != ImageStorage::Local
        && let ImageCloudTarget::ObjectStore(target) = &settings.cloud
    {
        target.validate()?;
    }
    if let Some(root) = &settings.root {
        ensure!(
            !root.trim().is_empty() && root.len() <= 4096 && !root.chars().any(char::is_control),
            "图片目录须为有效绝对路径"
        );
        ensure!(
            std::path::Path::new(root).is_absolute()
                && !std::path::Path::new(root)
                    .components()
                    .any(|part| matches!(part, std::path::Component::ParentDir)),
            "图片目录须为绝对路径"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_object_storage_configuration_roundtrips_without_secret_fields() {
        let public = serde_json::json!({
            "root": null, "storage": "both",
            "cloud": { "ObjectStore": {
                "endpoint": "https://storage.example.test",
                "bucket": "study-images", "region": "test-region-1",
                "access_key_id": "public-account-id", "prefix": "studyapp",
                "addressing": "virtual_hosted"
            }}
        });
        let settings: ImageSettings = serde_json::from_value(public.clone()).unwrap();
        let saved = serde_json::to_value(settings).unwrap();
        assert_eq!(saved["cloud"], public["cloud"]);
        assert!(saved.get("secret").is_none());
        assert!(saved.get("access_key_secret").is_none());
    }

    #[test]
    fn legacy_image_cloud_mode_explicitly_preserves_note_sync_compatibility() {
        let settings: ImageSettings = serde_json::from_str(r#"{"storage":"cloud"}"#).unwrap();
        assert_eq!(serde_json::to_value(settings).unwrap()["cloud"], "NoteSync");
    }

    #[test]
    fn cloud_object_target_requires_complete_public_configuration() {
        let settings: ImageSettings = serde_json::from_value(serde_json::json!({
            "storage": "cloud", "cloud": {"ObjectStore": {
                "endpoint": "https://objects.example.test", "bucket": "", "region": "", "access_key_id": "", "prefix": "", "addressing": "path"
            }}
        })).unwrap();
        assert!(validate_images(&settings).is_err());
    }
    #[test]
    fn legacy_images_default_to_local_ownership() {
        let images: ImageSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(images, ImageSettings::default());
    }
    #[test]
    fn custom_images_location_rejects_traversal_and_relative_paths() {
        for root in ["", "relative/images", "a/../images", "\0"] {
            assert!(
                validate_images(&ImageSettings {
                    root: Some(root.into()),
                    ..Default::default()
                })
                .is_err()
            );
        }
        let root = std::env::temp_dir().join("WWStudyImages");
        assert!(
            validate_images(&ImageSettings {
                root: Some(root.to_string_lossy().into_owned()),
                storage: ImageStorage::Both,
                ..Default::default()
            })
            .is_ok()
        );
        let traversal = root.join("..").join("elsewhere");
        assert!(
            validate_images(&ImageSettings {
                root: Some(traversal.to_string_lossy().into_owned()),
                ..Default::default()
            })
            .is_err()
        );
    }
}
