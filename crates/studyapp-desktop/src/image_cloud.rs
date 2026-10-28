//! Image cloud configuration and target-bound secrets, independent of note sync.

use anyhow::{Context as _, Result, ensure};
use serde::{Deserialize, Serialize};
use studyapp_core::{
    ImageSettings, ImageStorage,
    media_settings::{ImageCloudTarget, ObjectAddressing, ObjectStoreSettings},
};
use studyapp_sync::{AssetProvider, ObjectStoreConfig, ObjectStoreProvider};

#[derive(Clone, Default)]
pub enum SecretChange {
    #[default]
    Keep,
    Replace(zeroize::Zeroizing<String>),
    Clear,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ImageCredential {
    pub target: Option<ImageCloudTarget>,
    pub protected_secret: Vec<u8>,
}

fn provider_config(value: &ObjectStoreSettings) -> ObjectStoreConfig {
    ObjectStoreConfig {
        endpoint: value.endpoint.clone(),
        bucket: value.bucket.clone(),
        region: value.region.clone(),
        prefix: value.prefix.clone(),
        addressing: match value.addressing {
            ObjectAddressing::VirtualHosted => studyapp_sync::ObjectAddressing::VirtualHosted,
            ObjectAddressing::Path => studyapp_sync::ObjectAddressing::Path,
        },
    }
}

pub fn prepare(
    settings: &ImageSettings,
    change: SecretChange,
    previous: &ImageCredential,
) -> Result<ImageCredential> {
    studyapp_core::media_settings::validate_images(settings)?;
    if settings.storage != ImageStorage::Local
        && let ImageCloudTarget::ObjectStore(target) = &settings.cloud
    {
        ObjectStoreProvider::validate_config(&provider_config(target))?;
    }
    let protected_secret = match &settings.cloud {
        ImageCloudTarget::NoteSync => Vec::new(),
        ImageCloudTarget::ObjectStore(_) => match change {
            SecretChange::Replace(secret) => {
                ensure!(!secret.is_empty(), "请填写对象存储 Secret");
                crate::credentials::protect_image(&secret)?
            }
            SecretChange::Clear => Vec::new(),
            SecretChange::Keep if previous.target.as_ref() == Some(&settings.cloud) => {
                previous.protected_secret.clone()
            }
            SecretChange::Keep => {
                ensure!(
                    settings.storage == ImageStorage::Local,
                    "对象存储目标已改变，请重新填写 Secret 后保存"
                );
                Vec::new()
            }
        },
    };
    Ok(ImageCredential {
        target: Some(settings.cloud.clone()),
        protected_secret,
    })
}

pub fn validate_binding(settings: &ImageSettings, credentials: &ImageCredential) -> Result<()> {
    if let Some(target) = &credentials.target {
        ensure!(
            target == &settings.cloud,
            "图片凭据与云端目标不匹配；原配置已保留"
        );
    } else {
        ensure!(
            credentials.protected_secret.is_empty(),
            "图片凭据缺少对应目标"
        );
    }
    Ok(())
}

pub fn object_provider(
    settings: &ImageSettings,
    credentials: &ImageCredential,
) -> Result<Option<Box<dyn AssetProvider>>> {
    validate_binding(settings, credentials)?;
    let ImageCloudTarget::ObjectStore(target) = &settings.cloud else {
        return Ok(None);
    };
    target.validate()?;
    ensure!(
        !credentials.protected_secret.is_empty(),
        "对象存储尚未保存 Secret，请在图片云端配置中填写"
    );
    let secret = crate::credentials::unprotect_image(&credentials.protected_secret)
        .context("无法读取此用户保存的图片密钥，请重新填写 Secret")?;
    Ok(Some(Box::new(ObjectStoreProvider::new(
        provider_config(target),
        &target.access_key_id,
        &secret,
    )?)))
}

pub fn retry_with_provider(
    snapshot: &studyapp_core::Snapshot,
    provider: Result<Option<Box<dyn AssetProvider>>>,
) -> Result<String> {
    let provider = provider?.context("请先配置图片云端服务并保存设置")?;
    crate::media::retry_existing_assets(
        std::path::Path::new(&snapshot.data_path),
        &snapshot.settings.images,
        provider.as_ref(),
    )
}

use gpui_kit::{
    App, AppContext, Context, Entity, Window,
    component::input::{InputEvent, InputState},
};
use std::collections::BTreeMap;

pub struct ImageCloudDraft {
    pub fields: BTreeMap<&'static str, Entity<InputState>>,
    pub kind: u8,
    pub addressing: ObjectAddressing,
    pub clear_secret: bool,
    pub secret_saved: bool,
    pub picker: crate::folder_picker::FolderPicker,
}
impl ImageCloudDraft {
    pub fn new(
        settings: &ImageSettings,
        secret_saved: bool,
        window: &mut Window,
        cx: &mut Context<crate::app::StudyApp>,
    ) -> Self {
        let (kind, target) = match &settings.cloud {
            ImageCloudTarget::NoteSync => (1, ObjectStoreSettings::default()),
            ImageCloudTarget::ObjectStore(target) => (0, target.clone()),
        };
        let mut fields = BTreeMap::new();
        for (key, placeholder, value) in [
            ("endpoint", "https://objects.example.com", target.endpoint),
            ("bucket", "Bucket 名称", target.bucket),
            ("region", "服务商要求的 Region", target.region),
            ("access_key_id", "Access Key ID", target.access_key_id),
            ("prefix", "例如 studyapp；不加首尾斜杠", target.prefix),
            ("secret", "留空保留当前目标已保存的 Secret", String::new()),
        ] {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .default_value(value)
            });
            if key == "secret" {
                input.update(cx, |input, cx| input.set_masked(true, window, cx));
            }
            cx.subscribe(&input, |_, _, _: &InputEvent, cx| cx.notify())
                .detach();
            fields.insert(key, input);
        }
        Self {
            fields,
            kind,
            addressing: target.addressing,
            clear_secret: false,
            secret_saved,
            picker: Default::default(),
        }
    }
    pub fn target(&self, cx: &App) -> ImageCloudTarget {
        if self.kind == 1 {
            return ImageCloudTarget::NoteSync;
        }
        let value = |key: &str| self.fields[key].read(cx).value().trim().to_owned();
        ImageCloudTarget::ObjectStore(ObjectStoreSettings {
            endpoint: value("endpoint"),
            bucket: value("bucket"),
            region: value("region"),
            access_key_id: value("access_key_id"),
            prefix: value("prefix"),
            addressing: self.addressing,
        })
    }
    pub fn secret_change(&self, cx: &App) -> SecretChange {
        let secret = self.fields["secret"].read(cx).value().to_string();
        if !secret.is_empty() {
            SecretChange::Replace(zeroize::Zeroizing::new(secret))
        } else if self.clear_secret {
            SecretChange::Clear
        } else {
            SecretChange::Keep
        }
    }
    pub fn dirty(&self, baseline: &ImageSettings, cx: &App) -> bool {
        self.target(cx) != baseline.cloud || !matches!(self.secret_change(cx), SecretChange::Keep)
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn settings() -> studyapp_core::ImageSettings {
        studyapp_core::ImageSettings {
            storage: studyapp_core::ImageStorage::Both,
            cloud: studyapp_core::media_settings::ImageCloudTarget::ObjectStore(
                studyapp_core::media_settings::ObjectStoreSettings {
                    endpoint: "https://objects.example.test".into(),
                    bucket: "study-images".into(),
                    region: "us-east-1".into(),
                    access_key_id: "public-account".into(),
                    prefix: "studyapp".into(),
                    addressing: Default::default(),
                },
            ),
            ..Default::default()
        }
    }

    #[test]
    fn image_secret_is_encrypted_and_bound_to_exact_public_target() {
        let settings = settings();
        let saved = prepare(
            &settings,
            SecretChange::Replace(zeroize::Zeroizing::new("private-image-secret".into())),
            &ImageCredential::default(),
        )
        .unwrap();
        assert!(
            !serde_json::to_string(&saved)
                .unwrap()
                .contains("private-image-secret")
        );
        assert_eq!(
            crate::credentials::unprotect_image(&saved.protected_secret)
                .unwrap()
                .as_str(),
            "private-image-secret"
        );
        let kept = prepare(&settings, SecretChange::Keep, &saved).unwrap();
        assert_eq!(saved.protected_secret, kept.protected_secret);
        let mut changed = settings.clone();
        if let studyapp_core::media_settings::ImageCloudTarget::ObjectStore(target) =
            &mut changed.cloud
        {
            target.bucket = "another-bucket".into();
        }
        assert!(prepare(&changed, SecretChange::Keep, &saved).is_err());
        let cleared = prepare(&settings, SecretChange::Clear, &saved).unwrap();
        assert!(cleared.protected_secret.is_empty());
    }

    #[test]
    fn image_secret_context_cannot_be_misread_as_legacy_webdav_password() {
        let image = crate::credentials::protect_image("private").unwrap();
        assert!(crate::credentials::unprotect(&image).is_err());
        let legacy = crate::credentials::protect("legacy").unwrap();
        assert!(crate::credentials::unprotect_image(&legacy).is_err());
        assert_eq!(
            crate::credentials::unprotect(&legacy).unwrap().as_str(),
            "legacy"
        );
    }
}
