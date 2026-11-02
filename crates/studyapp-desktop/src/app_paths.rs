//! Application path registry. Reading never creates or repairs a locator.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};
#[derive(Clone, Debug)]
pub struct AppDirectories {
    pub data: PathBuf,
    pub config: PathBuf,
    pub local_data: PathBuf,
    pub cache: PathBuf,
    pub updates: PathBuf,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppPathConfig {
    pub schema: u32,
    pub root: PathBuf,
    #[serde(default)]
    pub library: Option<PathBuf>,
}
#[derive(Clone, Debug)]
pub struct AppPaths {
    pub defaults: AppDirectories,
    pub effective: AppDirectories,
    pub locator: PathBuf,
    pub custom_root: Option<PathBuf>,
    library: Option<PathBuf>,
    portable_editor: Option<PathBuf>,
}
impl AppPaths {
    pub fn current() -> Result<Self> {
        let p = directories::ProjectDirs::from("io", "StudyApp", "StudyApp")
            .context("无法定位应用目录")?;
        let defaults = AppDirectories {
            data: p.data_dir().into(),
            config: p.config_dir().into(),
            local_data: p.data_local_dir().into(),
            cache: p.cache_dir().into(),
            updates: std::env::temp_dir().join("WW-StudyApp-updates"),
        };
        let locator = defaults.config.join("storage-location.json");
        let paths = Self::from_defaults(defaults, locator)?;
        if cfg!(windows)
            && std::env::var_os("WW_STUDYAPP_PORTABLE_LAUNCHER").as_deref()
                == Some(std::ffi::OsStr::new("1"))
        {
            paths.with_portable_executable(&std::env::current_exe()?)
        } else {
            Ok(paths)
        }
    }
    /// Isolated native test profile; never reads the real user's locator.
    pub fn runtime() -> Result<Self> {
        if cfg!(test) || cfg!(feature = "ui-test") {
            let root =
                std::env::temp_dir().join(format!("studyapp-runtime-test-{}", std::process::id()));
            let defaults = AppDirectories {
                data: root.join("library"),
                config: root.join("config"),
                local_data: root.join("local"),
                cache: root.join("cache"),
                updates: root.join("updates"),
            };
            let locator = defaults.config.join("storage-location.json");
            Self::from_defaults(defaults, locator)
        } else {
            Self::current()
        }
    }
    pub fn configured_library(&self) -> Option<&Path> {
        self.library.as_deref()
    }
    pub fn from_defaults(defaults: AppDirectories, locator: PathBuf) -> Result<Self> {
        validate_path(&locator)?;
        let config = match std::fs::symlink_metadata(&locator) {
            Ok(_) => Some(validate_config(&read_json::<AppPathConfig>(&locator)?)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        let effective = if let Some(c) = &config {
            AppDirectories {
                data: c.root.join("library"),
                config: c.root.join("config"),
                local_data: c.root.join("local"),
                cache: c.root.join("cache"),
                updates: c.root.join("updates"),
            }
        } else {
            defaults.clone()
        };
        Ok(Self {
            defaults,
            effective,
            locator,
            custom_root: config.as_ref().map(|c| c.root.clone()),
            library: config.and_then(|c| c.library),
            portable_editor: None,
        })
    }
    pub fn with_portable_executable(mut self, executable: &Path) -> Result<Self> {
        let profile = executable
            .parent()
            .context("便携程序目录无效")?
            .join("webview-data");
        validate_path(&profile)?;
        self.portable_editor = Some(profile);
        Ok(self)
    }
    pub fn default_editor_profile(&self) -> PathBuf {
        self.portable_editor
            .clone()
            .unwrap_or_else(|| self.defaults.cache.join("editor"))
    }
    pub fn editor_profile(&self) -> PathBuf {
        if self.custom_root.is_some() {
            self.effective.cache.join("editor")
        } else {
            self.default_editor_profile()
        }
    }
    pub fn storage_locator(&self) -> &Path {
        &self.locator
    }
    pub fn legacy_location(&self) -> PathBuf {
        self.defaults.config.join("location.json")
    }
    pub fn resolve_library(&self, explicit: Option<&Path>) -> Result<PathBuf> {
        let result = if let Some(p) = explicit {
            p.into()
        } else if let Some(p) = &self.library {
            p.clone()
        } else {
            let legacy = self.legacy_location();
            match std::fs::symlink_metadata(&legacy) {
                Ok(_) => read_json::<PathBuf>(&legacy)?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => self.effective.data.clone(),
                Err(e) => return Err(e.into()),
            }
        };
        validate_path(&result)?;
        Ok(result)
    }
    /// Relocation must not detach an active or failed recovery transaction.
    pub fn verify_relocation_allowed(&self) -> Result<()> {
        for directory in [&self.defaults.local_data, &self.effective.local_data] {
            let pointer = directory.join("update-last.txt");
            validate_path(&pointer)?;
            if pointer.try_exists()? {
                ensure_terminal_pointer_for(&pointer, false)?;
            }
        }
        Ok(())
    }
    /// Publish a new transaction only after prior evidence is known to be terminal.
    /// Legacy recovery evidence remains at its original path after a root change.
    pub fn publish_update_pointer(&self, stage: &Path) -> Result<()> {
        validate_path(stage)?;
        let destination = self.effective.local_data.join("update-last.txt");
        let legacy = self.defaults.local_data.join("update-last.txt");
        for pointer in [&destination, &legacy] {
            validate_path(pointer)?;
            if pointer.try_exists()? {
                ensure_terminal_pointer(pointer)
                    .context("已有更新事务仍活跃或无法验证；请先恢复该事务")?;
            }
        }
        validate_path(&destination)?;
        std::fs::create_dir_all(&self.effective.local_data)
            .context("create update pointer parent")?;
        let _guards =
            guard_existing_parents(&destination).context("guard update pointer parent")?;
        let temporary = self
            .effective
            .local_data
            .join(format!("update-pointer-{}.tmp", uuid::Uuid::new_v4()));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .context("create temporary update pointer")?;
        file.write_all(
            stage
                .to_str()
                .context("更新目录需要有效文本路径")?
                .as_bytes(),
        )?;
        file.sync_all().context("sync update pointer")?;
        drop(file);
        std::fs::rename(&temporary, &destination).context("publish update pointer")?;
        Ok(())
    }
    /// Preserve the existing transaction's pointer; never transplant its contents.
    pub fn update_pointer(&self) -> Result<PathBuf> {
        let new = self.effective.local_data.join("update-last.txt");
        let old = self.defaults.local_data.join("update-last.txt");
        validate_path(&new)?;
        validate_path(&old)?;
        if new.try_exists()? || !old.try_exists()? {
            Ok(new)
        } else {
            Ok(old)
        }
    }
}
pub fn validate_config(config: &AppPathConfig) -> Result<AppPathConfig> {
    ensure!(config.schema == 1, "不支持的应用位置配置版本");
    validate_path(&config.root)?;
    if let Some(p) = &config.library {
        validate_path(p)?;
    }
    Ok(config.clone())
}
/// Validate all existing components; missing tails are permitted, links are not.
pub fn validate_path(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute() && !path.components().any(|c| matches!(c, Component::ParentDir)),
        "需要不含上级引用的绝对路径"
    );
    for p in path.ancestors() {
        match std::fs::symlink_metadata(p) {
            Ok(m) => {
                #[cfg(windows)]
                let link = {
                    use std::os::windows::fs::MetadataExt;
                    m.file_attributes() & 0x400 != 0
                };
                #[cfg(not(windows))]
                let link = m.file_type().is_symlink();
                ensure!(!link, "路径包含链接或重解析点：{}", p.display());
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| p.display().to_string()),
        }
    }
    Ok(())
}
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    serde_json::from_slice(&read_bounded(path)?).context("无法解析应用位置配置")
}
fn read_bounded(path: &Path) -> Result<Vec<u8>> {
    let _guards = guard_existing_parents(path)?;
    validate_path(path)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000);
        options.share_mode(1);
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
    let file: File = options.open(path)?;
    ensure!(
        file.metadata()?.is_file() && file.metadata()?.len() <= 65536,
        "位置配置文件无效或过大"
    );
    let mut bytes = Vec::new();
    file.take(65537).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 65536, "位置配置过大");
    Ok(bytes)
}

fn guard_existing_parents(path: &Path) -> Result<Vec<File>> {
    let mut guards = Vec::new();
    for parent in path.parent().into_iter().flat_map(Path::ancestors) {
        match std::fs::symlink_metadata(parent) {
            Ok(metadata) => {
                validate_path(parent)?;
                ensure!(metadata.is_dir(), "位置配置父路径不是目录");
                let mut options = std::fs::OpenOptions::new();
                options.read(true);
                #[cfg(windows)]
                {
                    use std::os::windows::fs::OpenOptionsExt;
                    options.custom_flags(0x0020_0000 | 0x0200_0000);
                    options.share_mode(3);
                }
                #[cfg(target_os = "linux")]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.custom_flags(0x20000 | 0x10000);
                }
                #[cfg(target_os = "macos")]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.custom_flags(0x100 | 0x100000);
                }
                guards.push(options.open(parent)?);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(guards)
}

fn ensure_terminal_pointer(pointer: &Path) -> Result<()> {
    ensure_terminal_pointer_for(pointer, true)
}
fn ensure_terminal_pointer_for(pointer: &Path, allow_failed: bool) -> Result<()> {
    let stage = PathBuf::from(String::from_utf8(read_bounded(pointer)?)?);
    validate_path(&stage)?;
    let transaction: serde_json::Value = read_json(&stage.join("transaction.json"))?;
    let transaction = transaction.get("transaction").unwrap_or(&transaction);
    ensure!(
        transaction["schema"] == 1 && transaction["id"].as_str().is_some_and(|s| !s.is_empty()),
        "更新事务无效"
    );
    let mut newest = None;
    let mut terminal = false;
    for (count, entry) in std::fs::read_dir(&stage)?.enumerate() {
        ensure!(count < 256, "更新事务记录过多");
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("status-") || !name.ends_with(".json") {
            continue;
        }
        let value: serde_json::Value = read_json(&entry.path())?;
        let state = value["state"].as_str().context("更新状态无效")?;
        ensure!(
            name.starts_with(&format!("status-{state}-")),
            "更新状态记录不匹配"
        );
        let is_terminal = matches!(state, "healthy" | "recovered" | "cancelled")
            || (allow_failed && state == "failed");
        let modified = entry.metadata()?.modified()?;
        match newest {
            None => {
                newest = Some(modified);
                terminal = is_terminal;
            }
            Some(previous) if modified > previous => {
                newest = Some(modified);
                terminal = is_terminal;
            }
            Some(previous) if modified == previous => {
                terminal &= is_terminal;
            }
            _ => {}
        }
    }
    ensure!(newest.is_some() && terminal, "更新尚未结束");
    Ok(())
}
