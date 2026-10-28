//! Read-only inspection; execution adapters are intentionally disconnected.
use anyhow::{Context, Result, ensure};
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek},
    path::{Path, PathBuf},
};
use studyapp_desktop::maintenance::*;

const MAX_JSON: u64 = 1024 * 1024;
#[derive(Clone, Debug)]
pub(crate) struct DiscoveryContext {
    pub default_library: PathBuf,
    pub location_file: Option<PathBuf>,
    pub explicit_library: Option<PathBuf>,
}
impl DiscoveryContext {
    pub fn current_user() -> Result<Self> {
        let dirs = directories::ProjectDirs::from("io", "StudyApp", "StudyApp")
            .context("无法定位用户数据目录")?;
        Ok(Self {
            default_library: dirs.data_dir().to_owned(),
            location_file: Some(dirs.config_dir().join("location.json")),
            explicit_library: None,
        })
    }
}
#[derive(Clone, Debug, Default)]
pub(crate) struct LibraryDiscovery {
    pub database_path: Option<PathBuf>,
    pub verified_database: Option<(PathBuf, FileIdentity)>,
    pub configuration_files: Vec<PathBuf>,
    pub issues: Vec<String>,
}
fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let _guards =
        crate::storage_backend::guard_directories(path.parent().context("文件目录无效")?, false)?;
    let mut file = crate::storage_backend::plain_handle(path, false, true)?;
    let (_, links) = handle_identity(&file, false)?;
    ensure!(links == 1, "文件存在多个链接");
    file.rewind()?;
    ensure!(file.metadata()?.len() <= MAX_JSON, "元数据超过容量限制");
    let mut bytes = vec![];
    file.take(MAX_JSON + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= MAX_JSON, "元数据超过容量限制");
    Ok(serde_json::from_slice(&bytes)?)
}
pub(crate) fn discover_library(context: &DiscoveryContext) -> LibraryDiscovery {
    let mut report = LibraryDiscovery::default();
    let resolve = || -> Result<PathBuf> {
        let library = if let Some(explicit) = &context.explicit_library {
            explicit.clone()
        } else if let Some(location) = &context.location_file {
            if location.try_exists()? {
                read_json(location)?
            } else {
                context.default_library.clone()
            }
        } else {
            context.default_library.clone()
        };
        absolute_components(&library)?;
        Ok(library)
    };
    let library = match resolve() {
        Ok(path) => path,
        Err(_) => {
            report
                .issues
                .push("资料库位置配置无法验证；已保留全部数据。".into());
            return report;
        }
    };
    let database = library.join("studyapp.sqlite3");
    report.database_path = Some(database.clone());
    if database.exists() {
        match observe_file(&database, "studyapp.sqlite3") {
            Ok(observed) if observed.link_count == 1 => {
                report.verified_database = Some((database, observed.identity))
            }
            _ => report
                .issues
                .push("数据库文件身份无法验证；已保留数据库。".into()),
        }
    }
    match configuration_inventory(
        &library,
        if context.explicit_library.is_none() {
            context.location_file.as_deref()
        } else {
            None
        },
    ) {
        Ok(files) => report.configuration_files = files,
        Err(_) => report
            .issues
            .push("部分配置无法验证；配置清理暂不可用。".into()),
    }
    report
}
fn revision_valid(name: &str) -> bool {
    name.starts_with("v1-")
        && name.len() <= 100
        && name
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b'v' || b == b'-')
}
fn push_existing(files: &mut Vec<PathBuf>, path: PathBuf) -> Result<()> {
    if path.try_exists()? {
        let observed = observe_file(&path, "configuration")?;
        ensure!(observed.link_count == 1, "配置有多个链接");
        files.push(path);
    }
    Ok(())
}
/// Enumerates only bounded, known settings resources. Unknown neighbors are retained.
pub(crate) fn configuration_inventory(
    library: &Path,
    location: Option<&Path>,
) -> Result<Vec<PathBuf>> {
    let mut files = vec![];
    let settings = library.join("settings");
    const CATEGORIES: &[&str] = &[
        "learning.json",
        "interface.json",
        "appearance.json",
        "images.json",
        "shortcuts.json",
        "sync.json",
    ];
    if settings.try_exists()? {
        let _guards = crate::storage_backend::guard_directories(&settings, false)?;
        let manifest_path = settings.join("manifest.json");
        if manifest_path.try_exists()? {
            let manifest: serde_json::Value = read_json(&manifest_path)?;
            ensure!(
                manifest["schema_version"].as_u64() == Some(1),
                "不支持的配置清单"
            );
            ensure!(
                manifest["current"].as_str().is_some_and(revision_valid),
                "配置版本无效"
            );
            if let Some(initial) = manifest.get("initial").filter(|v| !v.is_null()) {
                ensure!(initial.as_str().is_some_and(revision_valid), "配置版本无效");
            }
            if let Some(slot) = manifest.get("slot").filter(|v| !v.is_null()) {
                ensure!(slot.as_u64().is_some_and(|n| n <= 1), "配置槽位无效");
            }
            files.push(manifest_path);
        }
        for name in CATEGORIES {
            push_existing(&mut files, settings.join("defaults").join(name))?;
        }
        for slot in ["0", "1"] {
            let directory = settings.join("slots").join(slot);
            if directory.try_exists()? {
                let _guards = crate::storage_backend::guard_directories(&directory, false)?;
                let revision_path = directory.join("revision.json");
                let revision: String = read_json(&revision_path)?;
                ensure!(revision_valid(&revision), "配置槽位版本无效");
                for name in CATEGORIES
                    .iter()
                    .copied()
                    .chain(["credentials.json", "revision.json"])
                {
                    push_existing(&mut files, directory.join(name))?;
                }
            }
        }
        let generations = settings.join("generations");
        if generations.try_exists()? {
            let _guards = crate::storage_backend::guard_directories(&generations, false)?;
            for (count, entry) in std::fs::read_dir(&generations)?.enumerate() {
                ensure!(count < 256, "配置历史数量超过检查限制");
                let entry = entry?;
                if !entry.file_name().to_str().is_some_and(revision_valid) {
                    continue;
                }
                let directory = entry.path();
                let _guards = crate::storage_backend::guard_directories(&directory, false)?;
                for name in CATEGORIES.iter().copied().chain(["credentials.json"]) {
                    push_existing(&mut files, directory.join(name))?;
                }
            }
        }
    }
    push_existing(&mut files, library.join("sync-config.json"))?;
    // Resolve the library first, delete location metadata last in an executor.
    if let Some(location) = location
        && location.try_exists()?
    {
        let _: PathBuf = read_json(location)?;
        push_existing(&mut files, location.to_owned())?;
    }
    Ok(files)
}

pub(crate) fn handle_identity(file: &File, directory: bool) -> Result<(FileIdentity, u64)> {
    #[cfg(windows)]
    let (volume, file_id, links) = {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };
        let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
        // SAFETY: this borrowed handle remains live; output storage has API type/size.
        ensure!(
            unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) } != 0,
            "无法读取文件身份"
        );
        // SAFETY: successful API call initialized output.
        let info = unsafe { info.assume_init() };
        (
            u64::from(info.dwVolumeSerialNumber),
            (u128::from(info.nFileIndexHigh) << 32) | u128::from(info.nFileIndexLow),
            u64::from(info.nNumberOfLinks),
        )
    };
    #[cfg(unix)]
    let (volume, file_id, links) = {
        use std::os::unix::fs::MetadataExt;
        let m = file.metadata()?;
        (m.dev(), u128::from(m.ino()), m.nlink())
    };
    let metadata = file.metadata()?;
    if directory {
        ensure!(metadata.is_dir(), "目录身份类型不匹配");
        return Ok((
            FileIdentity {
                volume,
                file_id,
                size: 0,
                sha256: String::new(),
            },
            links,
        ));
    }
    ensure!(
        metadata.is_file() && metadata.len() <= 1024 * 1024 * 1024,
        "文件类型或大小不支持"
    );
    let mut hash = Sha256::new();
    let mut reader = file;
    let mut buffer = [0u8; 65536];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    ensure!(
        file.metadata()?.len() == metadata.len(),
        "读取期间文件已改变"
    );
    Ok((
        FileIdentity {
            volume,
            file_id,
            size: metadata.len(),
            sha256: format!("{:x}", hash.finalize()),
        },
        links,
    ))
}
pub(crate) fn observe_file(path: &Path, relative_path: &str) -> Result<ObservedFile> {
    let _guards =
        crate::storage_backend::guard_directories(path.parent().context("文件目录无效")?, false)?;
    let file = crate::storage_backend::plain_handle(path, false, true)?;
    let (identity, link_count) = handle_identity(&file, false)?;
    Ok(ObservedFile {
        relative_path: relative_path.into(),
        identity,
        is_reparse: false,
        link_count,
    })
}
/// No CREATE: merely opening/cancelling maintenance must not create a lock file.
/// Callers without an existing library lock must preserve data and retry later.
pub(crate) fn acquire_existing_library_lock(library: &Path) -> Result<File> {
    let _guards = crate::storage_backend::guard_directories(library, false)?;
    let lock = no_delete_file_handle(&library.join("studyapp.lock"))?;
    lock.try_lock()
        .context("资料库正在使用；请关闭应用后重试")?;
    Ok(lock)
}

/// A maintenance lease must pin the lock pathname as well as lock its bytes.
pub(crate) fn no_delete_file_handle(path: &Path) -> Result<File> {
    let _guards =
        crate::storage_backend::guard_directories(path.parent().context("文件目录无效")?, false)?;
    let checked = crate::storage_backend::plain_handle(path, false, true)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(3).custom_flags(0x0020_0000);
    }
    let mut file = options.open(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(
            file.metadata()?.file_attributes() & 0x400 == 0,
            "文件是重解析点"
        );
    }
    ensure!(handle_identity(&file, false)?.1 == 1, "文件存在多个链接");
    file.rewind()?;
    drop(checked);
    Ok(file)
}

#[derive(Clone, Debug)]
pub(crate) struct RegisteredMsi {
    pub product_code: String,
    pub upgrade_code: String,
    pub user_sid: String,
    pub install_root: PathBuf,
    pub version: String,
}
pub(crate) fn matching_msi_owner(
    executable: &Path,
    registrations: &[RegisteredMsi],
) -> Result<Option<InstallOwner>> {
    let name = executable
        .file_name()
        .and_then(|n| n.to_str())
        .context("应用文件名无效")?;
    ensure!(
        name.eq_ignore_ascii_case("WW-StudyApp.exe") || name.eq_ignore_ascii_case("Uninstall.exe"),
        "不是应用入口"
    );
    let root = executable.parent().context("安装目录无效")?;
    let mut owner = None;
    for registration in registrations {
        if !same_absolute_path(root, &registration.install_root)? {
            continue;
        }
        ensure!(
            owner.is_none()
                && registration
                    .upgrade_code
                    .eq_ignore_ascii_case(MSI_UPGRADE_CODE)
                && valid_product_code(&registration.product_code)
                && valid_user_sid(&registration.user_sid),
            "MSI 安装身份冲突"
        );
        owner = Some(InstallOwner::Msi {
            product_code: registration.product_code.clone(),
            upgrade_code: registration.upgrade_code.clone(),
            user_sid: registration.user_sid.clone(),
        });
    }
    Ok(owner)
}

#[derive(Clone, Debug)]
pub(crate) struct InstallDiscovery {
    pub receipt: Option<InstallReceipt>,
    pub library: LibraryDiscovery,
    pub issues: Vec<String>,
}
pub(crate) fn discover(executable: &Path, context: &DiscoveryContext) -> InstallDiscovery {
    discover_with_registrations(executable, context, registered_current_user_msi())
}
fn discover_with_registrations(
    executable: &Path,
    context: &DiscoveryContext,
    registrations: Result<Vec<RegisteredMsi>>,
) -> InstallDiscovery {
    let library = discover_library(context);
    let mut report = InstallDiscovery {
        receipt: None,
        library,
        issues: vec![],
    };
    let inspect = || -> Result<InstallReceipt> {
        let root = executable.parent().context("无法定位安装目录")?;
        let _guards = crate::storage_backend::guard_directories(root, false)?;
        let root_handle = crate::storage_backend::plain_handle(root, true, false)?;
        let root_identity = handle_identity(&root_handle, true)?.0;
        // A receipt cannot independently claim MSI ownership. The adapter must
        // match current-user MSI registration to this exact executable root.
        let registered = registrations?;
        let owner = matching_msi_owner(executable, &registered)?;
        let receipt: InstallReceipt = if let Some(owner @ InstallOwner::Msi { .. }) = &owner {
            let manifest: PackageManifest = read_json(&root.join("support/manifest.json"))?;
            validate_manifest(&manifest)?;
            let registration = registered
                .iter()
                .find(|r| same_absolute_path(root, &r.install_root).unwrap_or(false))
                .context("缺少 MSI 注册信息")?;
            ensure!(
                manifest.version == registration.version,
                "MSI 版本与资源清单不符"
            );
            InstallReceipt {
                schema: 1,
                install_id: registration.product_code.trim_matches(['{', '}']).into(),
                owner: owner.clone(),
                install_root: root.into(),
                root_identity: root_identity.clone(),
                version: manifest.version,
                source_package_sha256: String::new(),
                files: manifest.files,
            }
        } else {
            read_json(&root.join(RECEIPT_PATH))?
        };
        validate_receipt(&receipt)?;
        ensure!(
            same_absolute_path(root, &receipt.install_root)?,
            "安装目录与记录不符"
        );
        ensure!(root_identity == receipt.root_identity, "安装目录身份已改变");
        match (&receipt.owner, owner) {
            (InstallOwner::Portable, None) => {}
            (expected @ InstallOwner::Msi { .. }, Some(actual)) if *expected == actual => {}
            _ => anyhow::bail!("安装所有权无法验证"),
        }
        for resource in &receipt.files {
            if let Some(library) = report
                .library
                .database_path
                .as_ref()
                .and_then(|p| p.parent())
            {
                ensure!(
                    !absolute_components(&root.join(&resource.relative_path))?
                        .starts_with(&absolute_components(library)?),
                    "应用资源与资料库目录冲突"
                );
            }
            let actual =
                observe_file(&root.join(&resource.relative_path), &resource.relative_path)?;
            ensure!(
                actual.link_count == 1
                    && actual.identity.size == resource.size
                    && actual.identity.sha256 == resource.sha256,
                "应用资源已改变"
            );
        }
        let exe_name = executable
            .file_name()
            .and_then(|n| n.to_str())
            .context("应用文件名无效")?;
        ensure!(
            receipt
                .files
                .iter()
                .any(|f| f.relative_path.eq_ignore_ascii_case(exe_name)),
            "入口不属于安装记录"
        );
        Ok(receipt)
    };
    match inspect() {
        Ok(receipt) => report.receipt = Some(receipt),
        Err(_) => report
            .issues
            .push("无法验证此目录的安装记录。请使用原安装包修复，所有文件均已保留。".into()),
    }
    report
}

#[cfg(not(windows))]
pub(crate) fn registered_current_user_msi() -> Result<Vec<RegisteredMsi>> {
    Ok(vec![])
}
#[cfg(windows)]
pub(crate) fn registered_current_user_msi() -> Result<Vec<RegisteredMsi>> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "msi")]
    unsafe extern "system" {
        fn MsiEnumRelatedProductsW(
            upgrade: *const u16,
            reserved: u32,
            index: u32,
            product: *mut u16,
        ) -> u32;
        fn MsiGetProductInfoExW(
            product: *const u16,
            sid: *const u16,
            context: u32,
            property: *const u16,
            value: *mut u16,
            length: *mut u32,
        ) -> u32;
        fn MsiEnumProductsExW(
            product: *const u16,
            sid: *const u16,
            context: u32,
            index: u32,
            installed_product: *mut u16,
            installed_context: *mut u32,
            installed_sid: *mut u16,
            sid_length: *mut u32,
        ) -> u32;
    }
    fn wide(value: &str) -> Vec<u16> {
        std::ffi::OsStr::new(value)
            .encode_wide()
            .chain(Some(0))
            .collect()
    }
    fn property(product: &[u16], name: &str) -> Result<Option<String>> {
        let name = wide(name);
        let mut buffer = vec![0u16; 32768];
        let mut length = buffer.len() as u32;
        // SAFETY: NUL-terminated input strings, null SID selects current user;
        // context=2 is per-user unmanaged. Buffer length matches its allocation.
        let status = unsafe {
            MsiGetProductInfoExW(
                product.as_ptr(),
                std::ptr::null(),
                2,
                name.as_ptr(),
                buffer.as_mut_ptr(),
                &mut length,
            )
        };
        if status == 1605 || status == 1608 {
            return Ok(None);
        }
        ensure!(
            status == 0 && length < buffer.len() as u32,
            "无法读取当前用户 MSI 所有权"
        );
        Ok(Some(String::from_utf16(&buffer[..length as usize])?))
    }
    let upgrade = wide(MSI_UPGRADE_CODE);
    let mut registrations = vec![];
    for index in 0..128 {
        let mut product = [0u16; 39];
        // SAFETY: input is NUL-terminated GUID; output holds 38-char GUID + NUL.
        let status =
            unsafe { MsiEnumRelatedProductsW(upgrade.as_ptr(), 0, index, product.as_mut_ptr()) };
        if status == 259 {
            return Ok(registrations);
        }
        ensure!(status == 0, "无法枚举 MSI 所有权");
        let Some(location) = property(&product, "InstallLocation")? else {
            continue;
        };
        let Some(version) = property(&product, "VersionString")? else {
            continue;
        };
        let mut installed_product = [0u16; 39];
        let mut context = 0u32;
        let mut sid = [0u16; 185];
        let mut sid_length = sid.len() as u32;
        // SAFETY: current-user unmanaged enumeration, fixed product code and
        // writable buffers whose capacities match the passed lengths.
        let status = unsafe {
            MsiEnumProductsExW(
                product.as_ptr(),
                std::ptr::null(),
                2,
                0,
                installed_product.as_mut_ptr(),
                &mut context,
                sid.as_mut_ptr(),
                &mut sid_length,
            )
        };
        if status == 259 || status == 1605 {
            continue;
        }
        ensure!(
            status == 0
                && context == 2
                && installed_product == product
                && sid_length < sid.len() as u32,
            "无法读取 MSI 用户身份"
        );
        let user_sid = String::from_utf16(&sid[..sid_length as usize])?;
        if location.is_empty() {
            continue;
        }
        registrations.push(RegisteredMsi {
            product_code: String::from_utf16(&product[..38])?,
            upgrade_code: MSI_UPGRADE_CODE.into(),
            user_sid,
            install_root: PathBuf::from(location.trim_end_matches(['\\', '/'])),
            version,
        });
    }
    anyhow::bail!("MSI 注册数量超过检查限制")
}

/// Registration selects the update owner even when local payload metadata is damaged.
pub(crate) fn current_payload_kind(executable: &Path) -> Result<PayloadKind> {
    let registrations = registered_current_user_msi()?;
    Ok(
        if matching_msi_owner(executable, &registrations)?.is_some() {
            PayloadKind::Msi
        } else {
            PayloadKind::PortableZip
        },
    )
}

/// Read portable metadata while deferring per-resource conflict decisions to the planner.
pub(crate) fn read_portable_receipt(executable: &Path) -> Result<InstallReceipt> {
    ensure!(
        current_payload_kind(executable)? == PayloadKind::PortableZip,
        "此路径由 Windows Installer 管理"
    );
    let root = executable.parent().context("安装目录无效")?;
    let _guards = crate::storage_backend::guard_directories(root, false)?;
    let receipt: InstallReceipt = read_json(&root.join(RECEIPT_PATH))?;
    validate_receipt(&receipt)?;
    ensure!(
        receipt.owner == InstallOwner::Portable && same_absolute_path(root, &receipt.install_root)?,
        "便携版安装身份无效"
    );
    let handle = crate::storage_backend::plain_handle(root, true, false)?;
    ensure!(
        handle_identity(&handle, true)?.0 == receipt.root_identity,
        "安装目录身份已改变"
    );
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> PathBuf {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .join(".scratch/installation-20261026/test-temp")
            .join(format!("discovery-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }
    #[test]
    fn maintenance_read_only_missing_and_corrupt_location_never_materialize_library() {
        let root = fixture();
        let missing = root.join("missing");
        let config = root.join("location.json");
        let context = DiscoveryContext {
            default_library: missing.clone(),
            location_file: Some(config.clone()),
            explicit_library: None,
        };
        let initial = discover_library(&context);
        assert_eq!(
            initial.database_path,
            Some(missing.join("studyapp.sqlite3"))
        );
        assert!(initial.verified_database.is_none());
        assert!(!missing.exists());
        assert!(!config.exists());
        std::fs::write(&config, b"invalid json").unwrap();
        let corrupt = discover_library(&context);
        assert!(corrupt.database_path.is_none());
        assert_eq!(std::fs::read(&config).unwrap(), b"invalid json");
        assert!(!missing.exists());
    }
    #[test]
    fn maintenance_explicit_library_ignores_global_location_and_inventory_excludes_neighbors() {
        let root = fixture();
        let library = root.join("isolated");
        std::fs::create_dir(&library).unwrap();
        let config = root.join("location.json");
        std::fs::write(&config, b"broken").unwrap();
        let settings = library.join("settings");
        std::fs::create_dir(&settings).unwrap();
        std::fs::write(
            settings.join("manifest.json"),
            br#"{"schema_version":1,"current":"v1-1-2-3","slot":0}"#,
        )
        .unwrap();
        std::fs::write(settings.join("private-notes.txt"), b"keep").unwrap();
        let defaults = settings.join("defaults");
        std::fs::create_dir(&defaults).unwrap();
        std::fs::write(defaults.join("learning.json"), b"{}").unwrap();
        std::fs::write(defaults.join("foreign.json"), b"keep").unwrap();
        std::fs::write(library.join("studyapp.sqlite3"), b"fixture database").unwrap();
        let context = DiscoveryContext {
            default_library: root.join("default"),
            location_file: Some(config.clone()),
            explicit_library: Some(library.clone()),
        };
        let report = discover_library(&context);
        assert!(report.verified_database.is_some());
        assert!(
            report
                .configuration_files
                .contains(&defaults.join("learning.json"))
        );
        assert!(!report.configuration_files.contains(&config));
        assert!(
            !report
                .configuration_files
                .iter()
                .any(|p| p.ends_with("private-notes.txt") || p.ends_with("foreign.json"))
        );
        assert_eq!(std::fs::read(&config).unwrap(), b"broken");
        assert!(!library.join("studyapp.lock").exists());
    }
    #[test]
    fn maintenance_current_msi_directory_cannot_claim_portable_neighbor() {
        let owner = RegisteredMsi {
            product_code: "{FB2C8337-21E5-4406-83E2-4211C5691D1E}".into(),
            upgrade_code: MSI_UPGRADE_CODE.into(),
            user_sid: "S-1-5-21-1".into(),
            install_root: PathBuf::from(r"C:/Programs/Study"),
            version: "0.8.0".into(),
        };
        assert!(
            matching_msi_owner(
                std::path::Path::new(r"C:/Programs/Study/Uninstall.exe"),
                std::slice::from_ref(&owner)
            )
            .unwrap()
            .is_some()
        );
        assert!(
            matching_msi_owner(
                std::path::Path::new(r"C:/Portable/WW-StudyApp.exe"),
                &[owner]
            )
            .unwrap()
            .is_none()
        );
    }
    #[test]
    fn maintenance_msi_owner_rejects_missing_user_identity() {
        let owner = RegisteredMsi {
            product_code: "{FB2C8337-21E5-4406-83E2-4211C5691D1E}".into(),
            upgrade_code: MSI_UPGRADE_CODE.into(),
            user_sid: String::new(),
            install_root: PathBuf::from(r"C:/Programs/Study"),
            version: "0.8.0".into(),
        };
        assert!(
            matching_msi_owner(Path::new(r"C:/Programs/Study/Uninstall.exe"), &[owner]).is_err()
        );
    }
    #[test]
    fn maintenance_msi_observation_verifies_manifest_version_resources_and_library_separation() {
        let root = fixture();
        let installation = root.join("application");
        std::fs::create_dir(&installation).unwrap();
        std::fs::create_dir(installation.join("support")).unwrap();
        let executable = installation.join("Uninstall.exe");
        std::fs::write(&executable, b"test").unwrap();
        let observed = observe_file(&executable, "Uninstall.exe").unwrap();
        let manifest = PackageManifest {
            schema: 1,
            product: PRODUCT.into(),
            architecture: ARCHITECTURE.into(),
            version: "0.8.0".into(),
            files: vec![OwnedFile {
                relative_path: "Uninstall.exe".into(),
                role: FileRole::Uninstaller,
                size: 4,
                sha256: observed.identity.sha256,
            }],
        };
        std::fs::write(
            installation.join("support/manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let registration = RegisteredMsi {
            product_code: "{FB2C8337-21E5-4406-83E2-4211C5691D1E}".into(),
            upgrade_code: MSI_UPGRADE_CODE.into(),
            user_sid: "S-1-5-21-1".into(),
            install_root: installation.clone(),
            version: "0.8.0".into(),
        };
        let context = DiscoveryContext {
            default_library: root.join("missing-library"),
            location_file: None,
            explicit_library: None,
        };
        let inspect = |registration| {
            discover_with_registrations(&executable, &context, Ok(vec![registration]))
        };
        let observation = inspect(registration.clone()).receipt.unwrap();
        assert!(matches!(observation.owner, InstallOwner::Msi { .. }));
        assert!(observation.source_package_sha256.is_empty());
        assert!(!installation.join(RECEIPT_PATH).exists());
        assert!(!context.default_library.exists());
        assert!(
            inspect(RegisteredMsi {
                version: "0.7.0".into(),
                ..registration.clone()
            })
            .receipt
            .is_none()
        );
        assert!(
            inspect(RegisteredMsi {
                install_root: root.join("another-installation"),
                ..registration.clone()
            })
            .receipt
            .is_none()
        );
        let overlapping = DiscoveryContext {
            default_library: installation.clone(),
            location_file: None,
            explicit_library: None,
        };
        assert!(
            discover_with_registrations(&executable, &overlapping, Ok(vec![registration.clone()]))
                .receipt
                .is_none()
        );
        std::fs::write(&executable, b"modified").unwrap();
        assert!(inspect(registration).receipt.is_none());
    }
    #[test]
    fn maintenance_file_observation_captures_identity_and_read_only_lock_requires_existing_file() {
        let root = fixture();
        let file = root.join("app.dll");
        std::fs::write(&file, b"test").unwrap();
        let observed = observe_file(&file, "app.dll").unwrap();
        assert_eq!(observed.identity.size, 4);
        assert!(valid_sha256(&observed.identity.sha256));
        assert_ne!(observed.identity.file_id, 0);
        assert!(acquire_existing_library_lock(&root).is_err());
        assert!(!root.join("studyapp.lock").exists());
        std::fs::write(root.join("studyapp.lock"), b"").unwrap();
        let lock = acquire_existing_library_lock(&root).unwrap();
        assert!(acquire_existing_library_lock(&root).is_err());
        drop(lock);
    }
}
