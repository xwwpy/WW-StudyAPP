//! Categorized preferences. An atomic manifest replacement is the sole commit point.
use crate::sync_runtime::{
    CredentialPayload, PreparedSyncConfiguration, SyncRuntime, SyncSettings,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use studyapp_core::{Engine, LayoutSettings, PreparedSettings, Settings};

const MAX_JSON: u64 = 1024 * 1024;
const SCHEMA: u32 = 1;

#[derive(Clone)]
pub struct PreferenceSnapshot {
    pub revision: String,
    pub settings: Settings,
    pub shortcuts: BTreeMap<String, String>,
    pub sync: SyncSettings,
    pub image_secret_saved: bool,
}

impl PreferenceSnapshot {
    pub fn defaults() -> Self {
        Self {
            revision: String::new(),
            settings: Settings::default(),
            shortcuts: crate::shortcuts::default_bindings(),
            sync: SyncSettings::default(),
            image_secret_saved: false,
        }
    }
}

pub struct SavePreferences {
    pub expected_revision: String,
    pub settings: Settings,
    pub shortcuts: BTreeMap<String, String>,
    pub sync: SyncSettings,
    pub password: Option<zeroize::Zeroizing<String>>,
    pub image_secret: crate::image_cloud::SecretChange,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    schema_version: u32,
    current: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    initial: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    slot: Option<u8>,
}

#[derive(Serialize, Deserialize)]
struct Learning {
    interval_minutes: u32,
    answer_minutes: u32,
    review_days: Vec<u32>,
}
#[derive(Serialize, Deserialize)]
struct LearningFile {
    learning: Learning,
}
#[derive(Serialize, Deserialize)]
struct Interface {
    show_file_paths: bool,
    layout: LayoutSettings,
}
#[derive(Serialize, Deserialize)]
struct InterfaceFile {
    interface: Interface,
}
#[derive(Serialize, Deserialize)]
struct AppearanceFile {
    appearance: studyapp_core::AppearanceSettings,
}
#[derive(Serialize, Deserialize)]
struct ImagesFile {
    images: studyapp_core::ImageSettings,
}
#[derive(Serialize, Deserialize)]
struct ShortcutsFile {
    shortcuts: BTreeMap<String, String>,
}
#[derive(Serialize, Deserialize)]
struct SyncFile {
    sync: SyncSettings,
}

pub struct PreferencesStore {
    root: PathBuf,
    initial: Option<String>,
    current: PreferenceSnapshot,
    slot: Option<u8>,
}

pub struct PreparedPreferences {
    expected_revision: String,
    snapshot: PreferenceSnapshot,
    files: Vec<(&'static str, Vec<u8>)>,
    engine: PreparedSettings,
    synchronization: PreparedSyncConfiguration,
}

pub(crate) fn unique_revision() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!(
        "v1-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed),
    )
}

fn validate_revision(name: &str) -> Result<()> {
    ensure!(
        name.starts_with("v1-")
            && name.len() <= 100
            && name
                .bytes()
                .all(|b| b.is_ascii_digit() || b == b'v' || b == b'-'),
        "配置版本名称无效"
    );
    Ok(())
}

pub(crate) fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let _guards =
        crate::storage_backend::guard_directories(path.parent().context("配置目录不存在")?, false)?;
    let file = crate::storage_backend::plain_handle(path, false, true)?;
    ensure!(file.metadata()?.len() <= MAX_JSON, "配置文件超过容量限制");
    let mut bytes = Vec::new();
    file.take(MAX_JSON + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= MAX_JSON, "配置文件超过容量限制");
    serde_json::from_slice(&bytes).context("配置文件无法解析，原文件已保留")
}

fn read_category<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let _guards =
        crate::storage_backend::guard_directories(path.parent().context("配置目录不存在")?, false)?;
    let pinned = crate::storage_backend::plain_handle(path, false, true)?;
    ensure!(
        pinned.metadata()?.len() <= MAX_JSON,
        "配置分类文件超过容量限制"
    );
    config::Config::builder()
        .add_source(config::File::from(path))
        .build()
        .context("配置分类文件无法读取，原文件已保留")?
        .try_deserialize()
        .context("配置分类内容无效，原文件已保留")
}

fn category_files(snapshot: &PreferenceSnapshot) -> Result<Vec<(&'static str, Vec<u8>)>> {
    let settings = &snapshot.settings;
    Ok(vec![
        (
            "learning.json",
            serde_json::to_vec_pretty(&LearningFile {
                learning: Learning {
                    interval_minutes: settings.interval_minutes,
                    answer_minutes: settings.answer_minutes,
                    review_days: settings.review_days.clone(),
                },
            })?,
        ),
        (
            "interface.json",
            serde_json::to_vec_pretty(&InterfaceFile {
                interface: Interface {
                    show_file_paths: settings.show_file_paths,
                    layout: settings.layout.clone(),
                },
            })?,
        ),
        (
            "shortcuts.json",
            serde_json::to_vec_pretty(&ShortcutsFile {
                shortcuts: snapshot.shortcuts.clone(),
            })?,
        ),
        (
            "appearance.json",
            serde_json::to_vec_pretty(&AppearanceFile {
                appearance: settings.appearance.clone(),
            })?,
        ),
        (
            "images.json",
            serde_json::to_vec_pretty(&ImagesFile {
                images: settings.images.clone(),
            })?,
        ),
        (
            "sync.json",
            serde_json::to_vec_pretty(&SyncFile {
                sync: snapshot.sync.clone(),
            })?,
        ),
    ])
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    ensure!(bytes.len() as u64 <= MAX_JSON, "配置分类超过容量限制");
    let mut output = OpenOptions::new().write(true).create_new(true).open(path)?;
    output.write_all(bytes)?;
    output.sync_all()?;
    Ok(())
}

/// Only inactive slot files and the unpublished manifest candidate are reusable.
/// Open without truncation, validate the opened handle, then replace its bytes.
fn write_reusable(path: &Path, bytes: &[u8]) -> Result<()> {
    ensure!(bytes.len() as u64 <= MAX_JSON, "配置分类超过容量限制");
    let _guards =
        crate::storage_backend::guard_directories(path.parent().context("配置目录不存在")?, false)?;
    if path.try_exists()? {
        let _ = crate::storage_backend::plain_handle(path, false, false)?;
    }
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000).share_mode(1);
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0x20000);
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0x100);
    }
    let mut output = options.open(path)?;
    let metadata = output.metadata()?;
    ensure!(metadata.is_file(), "配置路径不是普通文件");
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };
        ensure!(
            metadata.file_attributes() & 0x400 == 0,
            "配置路径不能是重解析点"
        );
        let mut information = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
        // SAFETY: output owns this live file handle and information provides
        // enough writable storage for the API's output structure.
        if unsafe { GetFileInformationByHandle(output.as_raw_handle(), information.as_mut_ptr()) }
            == 0
        {
            return Err(std::io::Error::last_os_error()).context("无法检查配置文件链接数量");
        }
        // SAFETY: a successful API call initialized the entire structure.
        let information = unsafe { information.assume_init() };
        ensure!(
            information.nNumberOfLinks == 1,
            "配置事务文件不能包含硬链接"
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(metadata.nlink() == 1, "配置事务文件不能包含硬链接");
    }
    output.set_len(0)?;
    output.write_all(bytes)?;
    output.sync_all()?;
    Ok(())
}

impl PreferencesStore {
    pub fn open(database: &Path, engine: &mut Engine) -> Result<(Self, SyncRuntime)> {
        let root = database
            .parent()
            .context("资料库目录不存在")?
            .join("settings");
        let _guards = crate::storage_backend::guard_directories(&root, true)?;
        let manifest_path = root.join("manifest.json");
        let (store, runtime, prepared) = if manifest_path.try_exists()? {
            let (store, credentials) = Self::read(&root)?;
            let prepared = engine.prepare_settings(store.current.settings.clone())?;
            let runtime =
                SyncRuntime::from_configuration(database, store.current.sync.clone(), credentials)?;
            (store, runtime, prepared)
        } else {
            let (sync, credentials) = SyncRuntime::legacy_configuration(database)?;
            let current = PreferenceSnapshot {
                revision: unique_revision(),
                settings: engine.snapshot().settings,
                shortcuts: crate::shortcuts::default_bindings(),
                sync,
                image_secret_saved: !credentials.image.protected_secret.is_empty(),
            };
            current.settings.validate()?;
            crate::shortcuts::validate_bindings(&current.shortcuts)?;
            crate::sync_runtime::validate_settings(&current.sync)?;
            let mut store = Self {
                root,
                initial: None,
                current,
                slot: None,
            };
            let prepared = engine.prepare_settings(store.current.settings.clone())?;
            let runtime = SyncRuntime::from_configuration(
                database,
                store.current.sync.clone(),
                credentials.clone(),
            )?;
            store.write_defaults()?;
            let mut files = category_files(&store.current)?;
            files.push(("credentials.json", serde_json::to_vec_pretty(&credentials)?));
            store.write_slot(0, &store.current.revision, &files)?;
            store.replace_manifest(&store.current.revision, Some(0))?;
            store.slot = Some(0);
            (store, runtime, prepared)
        };
        engine.install_prepared_settings(prepared);
        store.clear_inactive_transaction();
        Ok((store, runtime))
    }

    fn write_defaults(&self) -> Result<()> {
        ensure!(
            !self.root.join("manifest.json").try_exists()?,
            "已建立的配置不能按首次导入重建默认源"
        );
        let defaults = self.root.join("defaults");
        let _guards = crate::storage_backend::guard_directories(&defaults, true)?;
        for (name, bytes) in category_files(&PreferenceSnapshot::defaults())? {
            let path = defaults.join(name);
            let exists = match std::fs::symlink_metadata(&path) {
                Ok(_) => {
                    let _ = crate::storage_backend::plain_handle(&path, false, false)?;
                    true
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error) => return Err(error.into()),
            };
            let expected: serde_json::Value = serde_json::from_slice(&bytes)?;
            if exists
                && read_category::<serde_json::Value>(&path).is_ok_and(|actual| actual == expected)
            {
                continue;
            }

            // Interrupted writes remain orphan temporary files, never partial
            // default sources. Keep the .json suffix for the config reader.
            let temporary = defaults.join(format!(".default-{}-{name}", unique_revision()));
            write_new(&temporary, &bytes)?;
            ensure!(
                read_category::<serde_json::Value>(&temporary)? == expected,
                "默认配置写入校验失败"
            );
            if exists {
                // Earlier versions wrote defaults directly to the final path.
                // Preserve exactly this incomplete source before replacing it;
                // this recovery is allowed only before the first manifest.
                let retained = defaults.join(format!(".retained-{name}-{}", unique_revision()));
                let _pinned = crate::storage_backend::plain_handle(&path, false, false)?;
                std::fs::rename(&path, &retained)
                    .context("无法保留中断的默认配置，原文件未改动")?;
            }
            std::fs::rename(&temporary, &path).context("默认配置发布失败；候选文件已保留")?;
        }
        Ok(())
    }

    fn read(root: &Path) -> Result<(Self, CredentialPayload)> {
        let manifest: Manifest = read_json(&root.join("manifest.json"))?;
        ensure!(
            manifest.schema_version == SCHEMA,
            "配置版本暂不受支持，原配置已保留"
        );
        validate_revision(&manifest.current)?;
        if let Some(initial) = &manifest.initial {
            validate_revision(initial)?;
        }
        let (current, credentials) = if let Some(slot) = manifest.slot {
            let directory = Self::slot_directory(root, slot)?;
            let revision: String = read_json(&directory.join("revision.json"))?;
            ensure!(revision == manifest.current, "配置槽位版本与清单不一致");
            Self::read_directory(&directory, &manifest.current, false)?
        } else {
            Self::read_generation(root, &manifest.current)?
        };
        // Existing libraries retain their immutable legacy source. New libraries
        // use only defaults + current and never create an initial generation.
        if let Some(initial) = &manifest.initial {
            if initial != &manifest.current || manifest.slot.is_some() {
                Self::read_generation(root, initial)?;
            }
        } else {
            for (name, _) in category_files(&PreferenceSnapshot::defaults())? {
                read_category::<serde_json::Value>(&root.join("defaults").join(name))?;
            }
        }
        Ok((
            Self {
                root: root.to_owned(),
                initial: manifest.initial,
                current,
                slot: manifest.slot,
            },
            credentials,
        ))
    }

    fn read_generation(
        root: &Path,
        revision: &str,
    ) -> Result<(PreferenceSnapshot, CredentialPayload)> {
        validate_revision(revision)?;
        let directory = root.join("generations").join(revision);
        Self::read_directory(&directory, revision, true)
    }

    fn slot_directory(root: &Path, slot: u8) -> Result<PathBuf> {
        ensure!(slot <= 1, "配置槽位无效");
        Ok(root.join("slots").join(slot.to_string()))
    }

    fn current_directory(&self) -> PathBuf {
        match self.slot {
            Some(slot) => Self::slot_directory(&self.root, slot).expect("validated slot"),
            None => self.root.join("generations").join(&self.current.revision),
        }
    }

    fn read_directory(
        directory: &Path,
        revision: &str,
        legacy: bool,
    ) -> Result<(PreferenceSnapshot, CredentialPayload)> {
        let learning: LearningFile = read_category(&directory.join("learning.json"))?;
        let interface: InterfaceFile = read_category(&directory.join("interface.json"))?;
        // A missing category is a pre-theme generation. Never write during load.
        let appearance = if !legacy || directory.join("appearance.json").try_exists()? {
            read_category::<AppearanceFile>(&directory.join("appearance.json"))?.appearance
        } else {
            studyapp_core::AppearanceSettings::default()
        };
        let images = if !legacy || directory.join("images.json").try_exists()? {
            read_category::<ImagesFile>(&directory.join("images.json"))?.images
        } else {
            studyapp_core::ImageSettings::default()
        };
        let shortcuts: ShortcutsFile = read_category(&directory.join("shortcuts.json"))?;
        let sync: SyncFile = read_category(&directory.join("sync.json"))?;
        let credentials: CredentialPayload = read_json(&directory.join("credentials.json"))?;
        let current = PreferenceSnapshot {
            revision: revision.to_owned(),
            settings: Settings {
                interval_minutes: learning.learning.interval_minutes,
                answer_minutes: learning.learning.answer_minutes,
                review_days: learning.learning.review_days,
                show_file_paths: interface.interface.show_file_paths,
                layout: interface.interface.layout,
                appearance,
                images,
            },
            shortcuts: crate::shortcuts::resolved_bindings(&shortcuts.shortcuts),
            sync: sync.sync,
            image_secret_saved: !credentials.image.protected_secret.is_empty(),
        };
        current.settings.validate()?;
        crate::image_cloud::validate_binding(&current.settings.images, &credentials.image)?;
        crate::shortcuts::validate_bindings(&current.shortcuts)?;
        crate::sync_runtime::validate_settings(&current.sync)?;
        ensure!(
            credentials.target == current.sync.target,
            "同步凭据与配置目标不匹配"
        );
        validate_revision(&credentials.target_epoch)?;
        Ok((current, credentials))
    }

    pub fn snapshot(&self) -> PreferenceSnapshot {
        self.current.clone()
    }

    pub fn prepare(
        &self,
        request: SavePreferences,
        engine: &Engine,
        runtime: &SyncRuntime,
    ) -> Result<PreparedPreferences> {
        ensure!(
            request.expected_revision == self.current.revision,
            "设置已被其他操作更新，请重新打开设置后再保存"
        );
        let engine = engine.prepare_settings(request.settings.clone())?;
        crate::shortcuts::validate_bindings(&request.shortcuts)?;
        let mut shortcuts = crate::shortcuts::default_bindings();
        for (action, binding) in request.shortcuts {
            shortcuts.insert(action, crate::shortcuts::normalize_binding(&binding)?);
        }
        let mut synchronization =
            runtime.prepare_configuration(request.sync.clone(), request.password)?;
        synchronization.credentials.image = crate::image_cloud::prepare(
            &request.settings.images,
            request.image_secret,
            runtime.image_credentials(),
        )?;
        let mut snapshot = PreferenceSnapshot {
            revision: unique_revision(),
            settings: request.settings,
            shortcuts,
            sync: request.sync,
            image_secret_saved: !synchronization
                .credentials
                .image
                .protected_secret
                .is_empty(),
        };
        let mut files = category_files(&snapshot)?;
        files.push((
            "credentials.json",
            serde_json::to_vec_pretty(&synchronization.credentials)?,
        ));
        for (_, bytes) in &files {
            ensure!(bytes.len() as u64 <= MAX_JSON, "配置分类超过容量限制");
        }
        let mut normalized_current = self.current.clone();
        for binding in normalized_current.shortcuts.values_mut() {
            *binding = crate::shortcuts::normalize_binding(binding)?;
        }
        let mut current_files = category_files(&normalized_current)?;
        let mut credentials: CredentialPayload =
            read_json(&self.current_directory().join("credentials.json"))?;
        credentials.image = crate::image_cloud::prepare(
            &self.current.settings.images,
            crate::image_cloud::SecretChange::Keep,
            &credentials.image,
        )?;
        current_files.push(("credentials.json", serde_json::to_vec_pretty(&credentials)?));
        if files == current_files {
            snapshot.revision = self.current.revision.clone();
        }
        Ok(PreparedPreferences {
            expected_revision: request.expected_revision,
            snapshot,
            files,
            engine,
            synchronization,
        })
    }

    #[cfg(test)]
    fn write_generation(&self, revision: &str, files: &[(&str, Vec<u8>)]) -> Result<()> {
        let generations = self.root.join("generations");
        let _guards = crate::storage_backend::guard_directories(&generations, true)?;
        let directory = generations.join(revision);
        std::fs::create_dir(&directory)?;
        let _generation_guard = crate::storage_backend::guard_directories(&directory, false)?;
        for (name, bytes) in files {
            write_new(&directory.join(name), bytes)?;
        }
        Self::read_generation(&self.root, revision)?;
        #[cfg(unix)]
        std::fs::File::open(&directory)?.sync_all()?;
        Ok(())
    }

    fn write_slot(&self, slot: u8, revision: &str, files: &[(&str, Vec<u8>)]) -> Result<()> {
        ensure!(self.slot != Some(slot), "不能覆盖正在使用的配置槽位");
        let directory = Self::slot_directory(&self.root, slot)?;
        let _guards = crate::storage_backend::guard_directories(&directory, true)?;
        for (name, bytes) in files {
            write_reusable(&directory.join(name), bytes)?;
        }
        Self::read_directory(&directory, revision, false)?;
        write_reusable(
            &directory.join("revision.json"),
            &serde_json::to_vec(revision)?,
        )?;
        #[cfg(unix)]
        std::fs::File::open(&directory)?.sync_all()?;
        Ok(())
    }

    fn clear_inactive_transaction(&self) {
        let Some(active) = self.slot else {
            return;
        };
        let clear = || -> Result<()> {
            let directory = Self::slot_directory(&self.root, 1 - active)?;
            if !directory.try_exists()? {
                return Ok(());
            }
            let _guards = crate::storage_backend::guard_directories(&directory, false)?;
            // Explicit fixed transaction paths only; never traverse or remove
            // user files, defaults, or any legacy generation.
            for name in [
                "learning.json",
                "interface.json",
                "appearance.json",
                "images.json",
                "shortcuts.json",
                "sync.json",
                "credentials.json",
                "revision.json",
            ] {
                let path = directory.join(name);
                if path.try_exists()? {
                    let handle = crate::storage_backend::plain_handle(&path, false, false)?;
                    let empty = handle.metadata()?.len() == 0;
                    drop(handle);
                    if !empty {
                        write_reusable(&path, &[])?;
                    }
                }
            }
            Ok(())
        };
        // Publication already succeeded: cleanup failure cannot turn a committed
        // save into a reported failure. The same fixed paths retry on reopen.
        if let Err(error) = clear() {
            eprintln!("设置已保存；事务暂存清空未完成，将在下次打开资料库时重试：{error:#}");
        }
    }

    fn replace_manifest(&self, revision: &str, slot: Option<u8>) -> Result<()> {
        let _guards = crate::storage_backend::guard_directories(&self.root, false)?;
        let manifest = self.root.join("manifest.json");
        if manifest.try_exists()? {
            // Refuse a symlink or special file before replacing the manifest.
            let _ = crate::storage_backend::plain_handle(&manifest, false, false)?;
        }
        let temporary = self.root.join(".manifest.next");
        write_reusable(
            &temporary,
            &serde_json::to_vec_pretty(&Manifest {
                schema_version: SCHEMA,
                current: revision.to_owned(),
                initial: self.initial.clone(),
                slot,
            })?,
        )?;
        std::fs::rename(&temporary, &manifest).context("设置发布失败；原设置和候选槽位均已保留")?;
        // Nothing fallible follows the sole publication point.
        Ok(())
    }

    fn publish(
        &mut self,
        prepared: PreparedPreferences,
    ) -> Result<(PreparedSettings, PreparedSyncConfiguration)> {
        ensure!(
            prepared.expected_revision == self.current.revision,
            "设置版本已改变，请重新保存"
        );
        let manifest: Manifest = read_json(&self.root.join("manifest.json"))?;
        ensure!(
            manifest.schema_version == SCHEMA && manifest.initial == self.initial,
            "磁盘配置清单已改变，请重新打开资料库"
        );
        ensure!(
            manifest.current == prepared.expected_revision,
            "磁盘设置版本已改变，请重新打开资料库"
        );
        ensure!(
            manifest.slot == self.slot,
            "磁盘配置槽位已改变，请重新打开资料库"
        );
        if prepared.snapshot.revision != self.current.revision {
            let slot = if self.slot == Some(0) { 1 } else { 0 };
            self.write_slot(slot, &prepared.snapshot.revision, &prepared.files)?;
            self.replace_manifest(&prepared.snapshot.revision, Some(slot))?;
            self.current = prepared.snapshot;
            self.slot = Some(slot);
        }
        Ok((prepared.engine, prepared.synchronization))
    }

    pub fn save(
        &mut self,
        request: SavePreferences,
        engine: &mut Engine,
        runtime: &mut SyncRuntime,
    ) -> Result<()> {
        let prepared = self.prepare(request, engine, runtime)?;
        let changed = prepared.snapshot.revision != self.current.revision;
        let (engine_settings, synchronization) = self.publish(prepared)?;
        if changed {
            engine.install_prepared_settings(engine_settings);
            runtime.install_configuration(synchronization);
            self.clear_inactive_transaction();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn object_store_secret_and_public_settings_publish_in_one_generation() {
        let (root, mut engine, mut runtime, mut store) = library();
        let old = store.snapshot();
        let mut proposal = request(&store);
        proposal.settings.images.storage = studyapp_core::ImageStorage::Both;
        proposal.settings.images.cloud =
            studyapp_core::media_settings::ImageCloudTarget::ObjectStore(
                studyapp_core::media_settings::ObjectStoreSettings {
                    endpoint: "https://objects.example.test".into(),
                    bucket: "study-images".into(),
                    region: "us-east-1".into(),
                    access_key_id: "account".into(),
                    prefix: "studyapp".into(),
                    addressing: Default::default(),
                },
            );
        proposal.image_secret = crate::image_cloud::SecretChange::Replace(zeroize::Zeroizing::new(
            "private-object-secret".into(),
        ));
        let prepared = store.prepare(proposal, &engine, &runtime).unwrap();
        assert_eq!(store.snapshot().revision, old.revision);
        assert!(
            !runtime
                .image_credentials()
                .protected_secret
                .iter()
                .any(|b| *b != 0)
        );
        let (settings, sync) = store.publish(prepared).unwrap();
        engine.install_prepared_settings(settings);
        runtime.install_configuration(sync);
        assert!(store.snapshot().image_secret_saved);
        let generation = store.current_directory();
        for name in ["images.json", "credentials.json", "sync.json"] {
            assert!(
                !std::fs::read_to_string(generation.join(name))
                    .unwrap()
                    .contains("private-object-secret")
            );
        }
        let (_, reopened) =
            PreferencesStore::open(&root.join("studyapp.sqlite3"), &mut engine).unwrap();
        assert_eq!(
            runtime.image_credentials().protected_secret,
            reopened.image_credentials().protected_secret
        );
        let original_secret = runtime.image_credentials().protected_secret.clone();
        let mut changed = request(&store);
        if let studyapp_core::media_settings::ImageCloudTarget::ObjectStore(target) =
            &mut changed.settings.images.cloud
        {
            target.bucket = "another-bucket".into();
        }
        assert!(store.save(changed, &mut engine, &mut runtime).is_err());
        assert_eq!(
            runtime.image_credentials().protected_secret,
            original_secret
        );
        let mut edit = request(&store);
        edit.settings.answer_minutes = 9;
        store.save(edit, &mut engine, &mut runtime).unwrap();
        assert_eq!(
            runtime.image_credentials().protected_secret,
            original_secret
        );
    }

    fn library() -> (PathBuf, Engine, SyncRuntime, PreferencesStore) {
        let root = tempfile::tempdir().unwrap().keep();
        let database = root.join("studyapp.sqlite3");
        let mut engine = Engine::open(&database, 1000).unwrap();
        let (store, runtime) = PreferencesStore::open(&database, &mut engine).unwrap();
        (root, engine, runtime, store)
    }

    fn legacy_library() -> (PathBuf, Engine, SyncRuntime, PreferencesStore) {
        let (root, engine, runtime, mut store) = library();
        let mut files = category_files(&store.current).unwrap();
        files.push((
            "credentials.json",
            std::fs::read(store.current_directory().join("credentials.json")).unwrap(),
        ));
        store
            .write_generation(&store.current.revision, &files)
            .unwrap();
        store.initial = Some(store.current.revision.clone());
        store
            .replace_manifest(&store.current.revision, None)
            .unwrap();
        store.slot = None;
        (root, engine, runtime, store)
    }

    fn request(store: &PreferencesStore) -> SavePreferences {
        let current = store.snapshot();
        SavePreferences {
            expected_revision: current.revision,
            settings: current.settings,
            shortcuts: current.shortcuts,
            sync: current.sync,
            password: None,
            image_secret: Default::default(),
        }
    }

    fn disk_files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut files = BTreeMap::new();
        for entry in std::fs::read_dir(root).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files.extend(disk_files(&path));
            } else {
                files.insert(path.clone(), std::fs::read(path).unwrap());
            }
        }
        files
    }

    #[test]
    fn unchanged_normalized_save_preserves_every_file_and_revision() {
        let (root, mut engine, mut runtime, mut store) = library();
        let before = disk_files(&root.join("settings"));
        let revision = store.snapshot().revision;
        let mut proposal = request(&store);
        proposal
            .shortcuts
            .insert("editor.save".into(), " ctrl+s ".into());
        store.save(proposal, &mut engine, &mut runtime).unwrap();
        assert_eq!(store.snapshot().revision, revision);
        assert_eq!(disk_files(&root.join("settings")), before);
    }

    #[test]
    fn new_library_keeps_only_defaults_and_one_effective_current_copy() {
        let (root, mut engine, mut runtime, mut store) = library();
        assert!(
            !root.join("settings/generations").exists(),
            "new libraries must not retain an initial copy of current settings"
        );
        for value in [6, 7, 8] {
            let mut proposal = request(&store);
            proposal.settings.answer_minutes = value;
            store.save(proposal, &mut engine, &mut runtime).unwrap();
            let inactive =
                PreferencesStore::slot_directory(&store.root, 1 - store.slot.unwrap()).unwrap();
            for entry in std::fs::read_dir(inactive).unwrap() {
                assert_eq!(
                    entry.unwrap().metadata().unwrap().len(),
                    0,
                    "inactive transaction contents must not become history"
                );
            }
        }
    }

    #[test]
    fn hardlinked_inactive_write_preserves_active_defaults_and_external_sources() {
        for target in ["active", "defaults", "external"] {
            let (root, mut engine, mut runtime, mut store) = library();
            let source = match target {
                "active" => store.current_directory().join("learning.json"),
                "defaults" => root.join("settings/defaults/learning.json"),
                _ => {
                    let source = root.join("external-preferences.json");
                    std::fs::write(&source, b"external configuration must remain intact").unwrap();
                    source
                }
            };
            let before = std::fs::read(&source).unwrap();
            let manifest = std::fs::read(store.root.join("manifest.json")).unwrap();
            let original = store.snapshot();
            let inactive =
                PreferencesStore::slot_directory(&store.root, 1 - store.slot.unwrap()).unwrap();
            std::fs::create_dir_all(&inactive).unwrap();
            std::fs::hard_link(&source, inactive.join("learning.json")).unwrap();
            let mut proposal = request(&store);
            proposal.settings.answer_minutes = 7;
            assert!(
                store.save(proposal, &mut engine, &mut runtime).is_err(),
                "hardlinked {target} was accepted for writing"
            );
            assert_eq!(
                std::fs::read(&source).unwrap(),
                before,
                "hardlinked {target} source was modified"
            );
            assert_eq!(
                std::fs::read(store.root.join("manifest.json")).unwrap(),
                manifest
            );
            assert_eq!(store.snapshot().revision, original.revision);
            assert_eq!(engine.snapshot().settings, original.settings);
        }
    }

    #[test]
    fn hardlinked_startup_cleanup_preserves_active_defaults_and_external_sources() {
        for target in ["active", "defaults", "external"] {
            let (root, mut engine, _, store) = library();
            let source = match target {
                "active" => store.current_directory().join("learning.json"),
                "defaults" => root.join("settings/defaults/learning.json"),
                _ => {
                    let source = root.join("external-preferences.json");
                    std::fs::write(&source, b"external configuration must remain intact").unwrap();
                    source
                }
            };
            let before = std::fs::read(&source).unwrap();
            let manifest = std::fs::read(store.root.join("manifest.json")).unwrap();
            let original = store.snapshot();
            let inactive =
                PreferencesStore::slot_directory(&store.root, 1 - store.slot.unwrap()).unwrap();
            std::fs::create_dir_all(&inactive).unwrap();
            std::fs::hard_link(&source, inactive.join("learning.json")).unwrap();
            let (reopened, _) =
                PreferencesStore::open(&root.join("studyapp.sqlite3"), &mut engine).unwrap();
            assert_eq!(
                std::fs::read(&source).unwrap(),
                before,
                "cleanup truncated hardlinked {target}"
            );
            assert_eq!(
                std::fs::read(store.root.join("manifest.json")).unwrap(),
                manifest
            );
            assert_eq!(reopened.snapshot().revision, original.revision);
            assert_eq!(engine.snapshot().settings, original.settings);
        }
    }

    #[test]
    fn hardlinked_manifest_candidate_cannot_modify_published_manifest() {
        let (_, mut engine, mut runtime, mut store) = library();
        let path = store.root.join("manifest.json");
        let before = std::fs::read(&path).unwrap();
        let original = store.snapshot();
        std::fs::hard_link(&path, store.root.join(".manifest.next")).unwrap();
        let mut proposal = request(&store);
        proposal.settings.answer_minutes = 7;
        assert!(store.save(proposal, &mut engine, &mut runtime).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(store.snapshot().revision, original.revision);
        assert_eq!(engine.snapshot().settings, original.settings);
    }

    #[test]
    fn reopen_clears_only_the_inactive_transaction_after_interrupted_cleanup() {
        let (root, mut engine, mut runtime, mut store) = library();
        let old = store.current_directory();
        let mut proposal = request(&store);
        proposal.settings.answer_minutes = 7;
        let prepared = store.prepare(proposal, &engine, &runtime).unwrap();
        let (settings, sync) = store.publish(prepared).unwrap();
        engine.install_prepared_settings(settings);
        runtime.install_configuration(sync);
        // Model interruption after commit/installation and before cleanup.
        assert!(
            !std::fs::read(old.join("credentials.json"))
                .unwrap()
                .is_empty()
        );
        let active = disk_files(&store.current_directory());
        let (reopened, _) =
            PreferencesStore::open(&root.join("studyapp.sqlite3"), &mut engine).unwrap();
        assert_eq!(reopened.snapshot().revision, store.snapshot().revision);
        assert_eq!(disk_files(&reopened.current_directory()), active);
        assert!(disk_files(&old).values().all(Vec::is_empty));
        assert!(!root.join("settings/generations").exists());
    }

    #[cfg(windows)]
    #[test]
    fn cleanup_failure_does_not_report_a_committed_save_as_failed_and_reopen_retries() {
        use std::os::windows::fs::OpenOptionsExt;
        let (root, mut engine, mut runtime, mut store) = library();
        let old = store.current_directory();
        let held = OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(old.join("credentials.json"))
            .unwrap();
        let mut proposal = request(&store);
        proposal.settings.answer_minutes = 7;
        store.save(proposal, &mut engine, &mut runtime).unwrap();
        assert_eq!(engine.snapshot().settings.answer_minutes, 7);
        assert!(
            !std::fs::read(old.join("credentials.json"))
                .unwrap()
                .is_empty()
        );
        let (reopened, _) =
            PreferencesStore::open(&root.join("studyapp.sqlite3"), &mut engine).unwrap();
        assert_eq!(reopened.snapshot().revision, store.snapshot().revision);
        drop(held);
        PreferencesStore::open(&root.join("studyapp.sqlite3"), &mut engine).unwrap();
        assert!(disk_files(&old).values().all(Vec::is_empty));
    }

    #[test]
    fn repeated_changed_saves_reuse_bounded_storage_and_unique_revisions_after_reopen() {
        let (root, mut engine, mut runtime, mut store) = library();
        let initial = disk_files(&root.join("settings/defaults"));
        let mut revisions = std::collections::BTreeSet::new();
        let mut bound = None;
        for index in 0..30 {
            let mut proposal = request(&store);
            proposal.settings.answer_minutes = 6 + index % 2;
            store.save(proposal, &mut engine, &mut runtime).unwrap();
            assert!(revisions.insert(store.snapshot().revision));
            let before_noop = disk_files(&root.join("settings"));
            store
                .save(request(&store), &mut engine, &mut runtime)
                .unwrap();
            assert_eq!(disk_files(&root.join("settings")), before_noop);
            if index == 3 {
                bound = Some(before_noop.keys().cloned().collect::<Vec<_>>());
            } else if index > 3 {
                assert_eq!(
                    before_noop.keys().cloned().collect::<Vec<_>>(),
                    *bound.as_ref().unwrap()
                );
            }
            let (reopened, sync) =
                PreferencesStore::open(&root.join("studyapp.sqlite3"), &mut engine).unwrap();
            assert_eq!(reopened.snapshot().revision, store.snapshot().revision);
            assert_eq!(reopened.snapshot().settings.answer_minutes, 6 + index % 2);
            store = reopened;
            runtime = sync;
        }
        for (path, bytes) in initial {
            if path.file_name().unwrap() != "manifest.json" {
                assert_eq!(std::fs::read(path).unwrap(), bytes);
            }
        }
        let files = disk_files(&root.join("settings"));
        assert!(files.values().map(Vec::len).sum::<usize>() < 64 * 1024);
    }

    #[test]
    fn prepared_request_stays_stale_when_its_slot_is_reused() {
        let (_, mut engine, mut runtime, mut store) = library();
        let mut first = request(&store);
        first.settings.answer_minutes = 6;
        store.save(first, &mut engine, &mut runtime).unwrap();
        let mut stale = request(&store);
        stale.settings.answer_minutes = 9;
        let stale = store.prepare(stale, &engine, &runtime).unwrap();
        let original_slot = store.slot;
        for value in [7, 8] {
            let mut proposal = request(&store);
            proposal.settings.answer_minutes = value;
            store.save(proposal, &mut engine, &mut runtime).unwrap();
        }
        assert_eq!(store.slot, original_slot);
        let before = disk_files(&store.root);
        assert!(store.publish(stale).is_err());
        assert_eq!(disk_files(&store.root), before);
        assert_eq!(store.snapshot().settings.answer_minutes, 8);
    }

    #[test]
    fn legacy_manifest_and_unreferenced_history_remain_unchanged_after_slot_saves() {
        let (root, mut engine, _, mut store) = legacy_library();
        let history_revision = unique_revision();
        let mut files = category_files(&store.current).unwrap();
        files.push((
            "credentials.json",
            std::fs::read(store.current_directory().join("credentials.json")).unwrap(),
        ));
        store.write_generation(&history_revision, &files).unwrap();
        let generations = disk_files(&root.join("settings/generations"));
        let (reopened, sync) =
            PreferencesStore::open(&root.join("studyapp.sqlite3"), &mut engine).unwrap();
        assert!(reopened.slot.is_none(), "old manifest has no slot field");
        store = reopened;
        let mut runtime = sync;
        for value in [6, 7, 8] {
            let mut proposal = request(&store);
            proposal.settings.answer_minutes = value;
            store.save(proposal, &mut engine, &mut runtime).unwrap();
        }
        assert_eq!(disk_files(&root.join("settings/generations")), generations);
    }

    #[test]
    fn malformed_slot_and_mismatched_slot_revision_are_rejected_without_repair() {
        let (root, mut engine, mut runtime, mut store) = library();
        let mut proposal = request(&store);
        proposal.settings.answer_minutes = 7;
        store.save(proposal, &mut engine, &mut runtime).unwrap();
        let manifest_path = root.join("settings/manifest.json");
        let original = std::fs::read(&manifest_path).unwrap();
        let mut manifest: serde_json::Value = serde_json::from_slice(&original).unwrap();
        manifest["slot"] = 9.into();
        std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let before = disk_files(&root.join("settings"));
        assert!(PreferencesStore::open(&root.join("studyapp.sqlite3"), &mut engine).is_err());
        assert_eq!(disk_files(&root.join("settings")), before);
        std::fs::write(&manifest_path, &original).unwrap();
        std::fs::write(
            store.current_directory().join("revision.json"),
            serde_json::to_vec(&unique_revision()).unwrap(),
        )
        .unwrap();
        let before = disk_files(&root.join("settings"));
        assert!(PreferencesStore::open(&root.join("studyapp.sqlite3"), &mut engine).is_err());
        assert_eq!(disk_files(&root.join("settings")), before);
    }

    #[cfg(windows)]
    #[test]
    fn repeated_manifest_failures_reuse_candidate_and_preserve_active_slot() {
        use std::os::windows::fs::OpenOptionsExt;
        let (root, mut engine, mut runtime, mut store) = library();
        for value in [6, 7] {
            let mut proposal = request(&store);
            proposal.settings.answer_minutes = value;
            store.save(proposal, &mut engine, &mut runtime).unwrap();
        }
        let original = store.snapshot();
        let active = disk_files(&store.current_directory());
        let manifest = root.join("settings/manifest.json");
        let before = std::fs::read(&manifest).unwrap();
        let held = OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&manifest)
            .unwrap();
        let mut paths = None;
        for _ in 0..12 {
            let mut proposal = request(&store);
            proposal.settings.answer_minutes = 9;
            assert!(store.save(proposal, &mut engine, &mut runtime).is_err());
            let files = disk_files(&root.join("settings"));
            let current_paths = files.keys().cloned().collect::<Vec<_>>();
            if let Some(paths) = &paths {
                assert_eq!(&current_paths, paths);
            }
            paths = Some(current_paths);
            assert!(files.values().map(Vec::len).sum::<usize>() < 64 * 1024);
            assert_eq!(disk_files(&store.current_directory()), active);
            assert_eq!(std::fs::read(&manifest).unwrap(), before);
            assert_eq!(store.snapshot().revision, original.revision);
            assert_eq!(engine.snapshot().settings, original.settings);
            let (reopened, _) =
                PreferencesStore::open(&root.join("studyapp.sqlite3"), &mut engine).unwrap();
            assert_eq!(reopened.snapshot().revision, original.revision);
        }
        drop(held);
        let mut proposal = request(&store);
        proposal.settings.answer_minutes = 9;
        store.save(proposal, &mut engine, &mut runtime).unwrap();
        assert_eq!(store.snapshot().settings.answer_minutes, 9);
    }

    #[test]
    fn appearance_is_a_separate_atomic_category_and_round_trips_only_after_save() {
        let (root, mut engine, mut runtime, mut store) = library();
        let old_revision = store.snapshot().revision;
        let initial = root.join("settings/defaults");
        assert!(initial.join("appearance.json").is_file());
        let mut value = serde_json::to_value(store.snapshot().settings).unwrap();
        value["appearance"] = serde_json::json!({"mode":"dark","font_size":16,"spacing":6});
        let mut proposal = request(&store);
        proposal.settings = serde_json::from_value(value).unwrap();
        let prepared = store.prepare(proposal, &engine, &runtime).unwrap();
        assert_eq!(store.snapshot().revision, old_revision);
        assert_eq!(
            serde_json::to_value(engine.snapshot().settings).unwrap()["appearance"]["mode"],
            "system"
        );
        let (settings, synchronization) = store.publish(prepared).unwrap();
        engine.install_prepared_settings(settings);
        runtime.install_configuration(synchronization);
        let (reopened, _) =
            PreferencesStore::open(&root.join("studyapp.sqlite3"), &mut engine).unwrap();
        assert_eq!(
            serde_json::to_value(reopened.snapshot().settings).unwrap()["appearance"]["mode"],
            "dark"
        );
        assert!(initial.join("appearance.json").is_file());
    }

    #[test]
    fn legacy_generation_without_appearance_category_loads_defaults_without_rewriting() {
        let (root, mut engine, _, store) = legacy_library();
        let manifest = root.join("settings/manifest.json");
        let before = std::fs::read(&manifest).unwrap();
        let category = store.current_directory().join("appearance.json");
        std::fs::remove_file(&category).unwrap();
        let (reopened, _) =
            PreferencesStore::open(&root.join("studyapp.sqlite3"), &mut engine).unwrap();
        assert_eq!(
            reopened.snapshot().settings.appearance,
            studyapp_core::AppearanceSettings::default()
        );
        assert!(!category.exists());
        assert_eq!(std::fs::read(manifest).unwrap(), before);
    }

    #[test]
    fn damaged_appearance_is_rejected_and_preserved_instead_of_using_defaults() {
        let (root, mut engine, _, store) = library();
        let category = store.current_directory().join("appearance.json");
        std::fs::write(&category, "{ invalid appearance retained").unwrap();
        assert!(PreferencesStore::open(&root.join("studyapp.sqlite3"), &mut engine).is_err());
        assert_eq!(
            std::fs::read_to_string(category).unwrap(),
            "{ invalid appearance retained"
        );
    }

    #[test]
    fn custom_image_root_and_storage_survive_category_save_and_restart() {
        let (root, mut engine, mut runtime, mut store) = library();
        let custom_root = root
            .join("custom-image-assets")
            .to_string_lossy()
            .into_owned();
        let expected = studyapp_core::ImageSettings {
            root: Some(custom_root),
            storage: studyapp_core::ImageStorage::Both,
            ..Default::default()
        };
        let mut proposal = request(&store);
        proposal.settings.images = expected.clone();
        store.save(proposal, &mut engine, &mut runtime).unwrap();
        let category = store.current_directory().join("images.json");
        let written: ImagesFile = read_category(&category).unwrap();
        assert_eq!(written.images, expected);
        let mut unrelated = request(&store);
        unrelated.settings.answer_minutes = 9;
        store.save(unrelated, &mut engine, &mut runtime).unwrap();
        let (reopened, _) =
            PreferencesStore::open(&root.join("studyapp.sqlite3"), &mut engine).unwrap();
        assert_eq!(reopened.snapshot().settings.images, expected);
        assert_eq!(engine.snapshot().settings.images, expected);
    }

    #[test]
    fn interrupted_initial_defaults_are_retained_and_rebuilt_before_manifest_publication() {
        for partial in ["{", "{}"] {
            let root = tempfile::tempdir().unwrap().keep();
            let database = root.join("studyapp.sqlite3");
            let mut engine = Engine::open(&database, 1000).unwrap();
            let defaults = root.join("settings/defaults");
            std::fs::create_dir_all(&defaults).unwrap();
            let damaged = defaults.join("learning.json");
            std::fs::write(&damaged, partial).unwrap();
            let (store, _) = PreferencesStore::open(&database, &mut engine)
                .unwrap_or_else(|error| panic!("interrupted import must recover: {error:#}"));
            assert_eq!(store.snapshot().settings, Settings::default());
            let expected: serde_json::Value = serde_json::from_slice(
                &category_files(&PreferenceSnapshot::defaults()).unwrap()[0].1,
            )
            .unwrap();
            assert_eq!(
                read_category::<serde_json::Value>(&damaged).unwrap(),
                expected
            );
            let retained: Vec<_> = std::fs::read_dir(&defaults)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .filter(|path| {
                    path.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with(".retained-learning.json-")
                })
                .collect();
            assert_eq!(retained.len(), 1);
            assert_eq!(std::fs::read_to_string(&retained[0]).unwrap(), partial);
            let (reopened, _) = PreferencesStore::open(&database, &mut engine).unwrap();
            assert_eq!(reopened.snapshot().revision, store.snapshot().revision);
            assert_eq!(std::fs::read_to_string(&retained[0]).unwrap(), partial);
        }
    }

    #[test]
    fn damaged_existing_manifest_is_not_recovered_as_an_interrupted_import() {
        let (root, mut engine, _, _) = library();
        let manifest = root.join("settings/manifest.json");
        let default = root.join("settings/defaults/learning.json");
        std::fs::write(&manifest, "{").unwrap();
        std::fs::write(&default, "{").unwrap();
        assert!(PreferencesStore::open(&root.join("studyapp.sqlite3"), &mut engine).is_err());
        assert_eq!(std::fs::read_to_string(manifest).unwrap(), "{");
        assert_eq!(std::fs::read_to_string(default).unwrap(), "{");
    }

    #[test]
    fn import_legacy_settings_and_sync_preserves_original_sources() {
        let root = tempfile::tempdir().unwrap().keep();
        let database = root.join("studyapp.sqlite3");
        let mut engine = Engine::open(&database, 1000).unwrap();
        let mut settings = engine.snapshot().settings;
        settings.answer_minutes = 9;
        engine
            .apply(studyapp_core::Command::SaveSettings(settings.clone()), 1000)
            .unwrap();
        let legacy = br#"{"settings":{"target":"Disabled","auto_minutes":30},"protected_password":[],"baselines":{},"last_success":42}"#;
        std::fs::write(root.join("sync-config.json"), legacy).unwrap();
        let (store, runtime) = PreferencesStore::open(&database, &mut engine).unwrap();
        assert_eq!(store.snapshot().settings, settings);
        assert_eq!(store.snapshot().sync.auto_minutes, Some(30));
        assert_eq!(runtime.status.last_success, Some(42));
        assert_eq!(
            std::fs::read(root.join("sync-config.json")).unwrap(),
            legacy
        );
        assert!(root.join("settings/defaults/learning.json").is_file());
        assert!(root.join("settings/defaults/interface.json").is_file());
        assert!(root.join("settings/defaults/shortcuts.json").is_file());
        assert!(root.join("settings/defaults/sync.json").is_file());
        assert!(!root.join("settings/defaults/credentials.json").exists());
        for (name, bytes) in category_files(&PreferenceSnapshot::defaults()).unwrap() {
            let actual: serde_json::Value =
                read_category(&root.join("settings/defaults").join(name)).unwrap();
            let expected: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(
                actual, expected,
                "UI defaults and disk defaults must share one source"
            );
        }
    }

    #[test]
    fn prepare_is_read_only_and_stale_revision_cannot_publish() {
        let (root, engine, runtime, mut store) = library();
        let before = std::fs::read(root.join("settings/manifest.json")).unwrap();
        let count = disk_files(&root.join("settings")).len();
        let mut proposal = request(&store);
        proposal.settings.answer_minutes = 7;
        let prepared = store.prepare(proposal, &engine, &runtime).unwrap();
        assert_eq!(
            std::fs::read(root.join("settings/manifest.json")).unwrap(),
            before
        );
        assert_eq!(disk_files(&root.join("settings")).len(), count);
        assert_eq!(engine.snapshot().settings.answer_minutes, 5);
        let stale = request(&store);
        store.publish(prepared).unwrap();
        assert!(store.prepare(stale, &engine, &runtime).is_err());
    }

    #[test]
    fn invalid_settings_and_interrupted_generation_never_replace_committed_preferences() {
        let (root, mut engine, mut runtime, mut store) = library();
        let original = store.snapshot();
        let manifest = std::fs::read(root.join("settings/manifest.json")).unwrap();
        let mut invalid = request(&store);
        invalid.settings.interval_minutes = 0;
        assert!(store.save(invalid, &mut engine, &mut runtime).is_err());
        let mut valid = request(&store);
        valid.settings.answer_minutes = 8;
        let prepared = store.prepare(valid, &engine, &runtime).unwrap();
        // A blocked inactive slot cannot affect the currently published source.
        let candidate =
            PreferencesStore::slot_directory(&store.root, 1 - store.slot.unwrap()).unwrap();
        std::fs::create_dir_all(&candidate).unwrap();
        std::fs::create_dir(candidate.join("learning.json")).unwrap();
        assert!(store.publish(prepared).is_err());
        assert_eq!(store.snapshot().revision, original.revision);
        assert_eq!(engine.snapshot().settings, original.settings);
        assert_eq!(
            std::fs::read(root.join("settings/manifest.json")).unwrap(),
            manifest
        );
        let (reopened, _) =
            PreferencesStore::open(&root.join("studyapp.sqlite3"), &mut engine).unwrap();
        assert_eq!(reopened.snapshot().revision, original.revision);
    }

    #[test]
    fn synchronization_state_is_separate_and_only_reused_for_its_target_epoch() {
        let (root, mut engine, mut runtime, mut store) = library();
        let folder = root.join("remote");
        std::fs::create_dir(&folder).unwrap();
        let mut proposal = request(&store);
        proposal.sync.target = crate::sync_runtime::SyncTarget::Folder {
            path: folder.to_string_lossy().into_owned(),
        };
        store.save(proposal, &mut engine, &mut runtime).unwrap();
        let completion = runtime
            .task(std::sync::Arc::new(engine.snapshot()), false)
            .unwrap()()
        .map_err(|e| e.to_string());
        runtime.finish(completion, &mut engine, 1100).unwrap();
        let state_path = root.join("sync-state.json");
        let state: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
        assert_eq!(state["last_success"], 1100);
        assert!(state.get("protected_password").is_none());
        assert!(state.get("settings").is_none());
        let mut proposal = request(&store);
        proposal.settings.answer_minutes = 9;
        store.save(proposal, &mut engine, &mut runtime).unwrap();
        assert_eq!(runtime.status.last_success, Some(1100));
        let (_, reopened) =
            PreferencesStore::open(&root.join("studyapp.sqlite3"), &mut engine).unwrap();
        assert_eq!(reopened.status.last_success, Some(1100));
        let mut proposal = request(&store);
        proposal.sync.target = crate::sync_runtime::SyncTarget::Folder {
            path: root.join("other-remote").to_string_lossy().into_owned(),
        };
        store.save(proposal, &mut engine, &mut runtime).unwrap();
        assert_eq!(runtime.status.last_success, None);
        let (_, reopened) =
            PreferencesStore::open(&root.join("studyapp.sqlite3"), &mut engine).unwrap();
        assert_eq!(
            reopened.status.last_success, None,
            "old target baseline must not load"
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(state_path).unwrap())
                .unwrap(),
            state
        );
    }

    #[cfg(windows)]
    #[test]
    fn credentials_are_ciphertext_and_unrelated_edits_keep_original_ciphertext() {
        let (_, mut engine, mut runtime, mut store) = library();
        let secret = "preferences-test-secret-should-never-be-json";
        let mut proposal = request(&store);
        proposal.sync.target = crate::sync_runtime::SyncTarget::WebDav {
            url: "https://example.com/dav/".into(),
            username: "learner".into(),
        };
        proposal.password = Some(zeroize::Zeroizing::new(secret.into()));
        store.save(proposal, &mut engine, &mut runtime).unwrap();
        let generation = store.current_directory();
        for name in [
            "learning.json",
            "interface.json",
            "shortcuts.json",
            "sync.json",
            "credentials.json",
        ] {
            assert!(
                !std::fs::read_to_string(generation.join(name))
                    .unwrap()
                    .contains(secret)
            );
        }
        let before: CredentialPayload = read_json(&generation.join("credentials.json")).unwrap();
        assert_eq!(
            crate::credentials::unprotect(&before.protected_password)
                .unwrap()
                .as_str(),
            secret
        );
        let mut proposal = request(&store);
        proposal.settings.answer_minutes = 9;
        store.save(proposal, &mut engine, &mut runtime).unwrap();
        let after: CredentialPayload =
            read_json(&store.current_directory().join("credentials.json")).unwrap();
        assert_eq!(after.protected_password, before.protected_password);
        assert_eq!(after.target_epoch, before.target_epoch);
        let mut changed = request(&store);
        changed.sync.target = crate::sync_runtime::SyncTarget::WebDav {
            url: "https://example.org/dav/".into(),
            username: "learner".into(),
        };
        assert!(store.save(changed, &mut engine, &mut runtime).is_err());
    }

    #[test]
    fn categorized_publication_replays_on_restart_and_is_library_scoped() {
        let (root, mut engine, mut runtime, mut store) = library();
        let (_, _, _, other) = library();
        let mut proposal = request(&store);
        proposal.settings.answer_minutes = 7;
        proposal.settings.layout.folders_visible = false;
        proposal.settings.show_file_paths = true;
        store.save(proposal, &mut engine, &mut runtime).unwrap();
        assert_eq!(engine.snapshot().settings.answer_minutes, 7);
        let expected = store.snapshot();
        let folder = store.current_directory();
        let interface: InterfaceFile = read_category(&folder.join("interface.json")).unwrap();
        assert!(interface.interface.show_file_paths);
        assert!(!interface.interface.layout.folders_visible);
        // The database mirror is deliberately stale until a later state checkpoint.
        let mut reopened = Engine::open(&root.join("studyapp.sqlite3"), 1001).unwrap();
        let (replayed, _) =
            PreferencesStore::open(&root.join("studyapp.sqlite3"), &mut reopened).unwrap();
        assert_eq!(replayed.snapshot().revision, expected.revision);
        assert_eq!(reopened.snapshot().settings, expected.settings);
        assert_eq!(other.snapshot().settings.answer_minutes, 5);
        assert_ne!(other.snapshot().revision, expected.revision);
    }

    #[test]
    fn malformed_generation_name_is_rejected_without_reimport() {
        let (root, mut engine, _, _) = library();
        let path = root.join("settings/manifest.json");
        let mut manifest: Manifest = read_json(&path).unwrap();
        manifest.current = "../escape".into();
        std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(PreferencesStore::open(&root.join("studyapp.sqlite3"), &mut engine).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn oversized_or_damaged_category_is_rejected_without_rewriting_the_manifest() {
        let (root, mut engine, _, store) = library();
        let manifest_path = root.join("settings/manifest.json");
        let before = std::fs::read(&manifest_path).unwrap();
        let category = store.current_directory().join("interface.json");
        std::fs::OpenOptions::new()
            .write(true)
            .open(&category)
            .unwrap()
            .set_len(MAX_JSON + 1)
            .unwrap();
        assert!(PreferencesStore::open(&root.join("studyapp.sqlite3"), &mut engine).is_err());
        assert_eq!(std::fs::read(&manifest_path).unwrap(), before);
        std::fs::write(&category, "{ broken source retained").unwrap();
        assert!(PreferencesStore::open(&root.join("studyapp.sqlite3"), &mut engine).is_err());
        assert_eq!(
            std::fs::read_to_string(category).unwrap(),
            "{ broken source retained"
        );
        assert_eq!(std::fs::read(manifest_path).unwrap(), before);
    }

    #[cfg(windows)]
    #[test]
    fn failed_manifest_replace_preserves_disk_and_live_configuration() {
        use std::os::windows::fs::OpenOptionsExt;
        let (root, mut engine, mut runtime, mut store) = library();
        let path = root.join("settings/manifest.json");
        let before = std::fs::read(&path).unwrap();
        let original = store.snapshot();
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&path)
            .unwrap();
        let mut proposal = request(&store);
        proposal.settings.answer_minutes = 7;
        assert!(store.save(proposal, &mut engine, &mut runtime).is_err());
        assert_eq!(store.snapshot().revision, original.revision);
        assert_eq!(engine.snapshot().settings, original.settings);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        drop(held);
        let (replayed, _) =
            PreferencesStore::open(&root.join("studyapp.sqlite3"), &mut engine).unwrap();
        assert_eq!(replayed.snapshot().revision, original.revision);
    }
}
