//! Validated Windows resource names. A receipt supplies evidence, never deletion authority.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

pub const PRODUCT: &str = "WW-StudyApp";
pub const ARCHITECTURE: &str = "x86_64";
pub const MSI_UPGRADE_CODE: &str = "{4CF5A7C2-094D-4D97-AB25-24C6C2BAA4CD}";
pub const RECEIPT_PATH: &str = "support/install-receipt.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum InstallOwner {
    Portable,
    Msi {
        product_code: String,
        upgrade_code: String,
        user_sid: String,
    },
    LegacyPortable,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PayloadKind {
    LegacyZip,
    PortableZip,
    Msi,
    Setup,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileRole {
    Application,
    Uninstaller,
    Runtime,
    Notice,
    Manifest,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnedFile {
    pub relative_path: String,
    pub role: FileRole,
    pub size: u64,
    pub sha256: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageManifest {
    pub schema: u32,
    pub product: String,
    pub version: String,
    pub architecture: String,
    pub files: Vec<OwnedFile>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileIdentity {
    pub volume: u64,
    pub file_id: u128,
    pub size: u64,
    pub sha256: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallReceipt {
    pub schema: u32,
    pub install_id: String,
    pub owner: InstallOwner,
    pub install_root: PathBuf,
    pub root_identity: FileIdentity,
    pub version: String,
    /// Empty only for an MSI registration observation, which is never portable
    /// execution authority and makes no claim about an archive digest.
    pub source_package_sha256: String,
    pub files: Vec<OwnedFile>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservedFile {
    pub relative_path: String,
    pub identity: FileIdentity,
    pub is_reparse: bool,
    pub link_count: u64,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProtectedPaths {
    pub files: Vec<PathBuf>,
    pub directories: Vec<PathBuf>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileOperation {
    Add {
        incoming: OwnedFile,
    },
    Replace {
        old: OwnedFile,
        incoming: OwnedFile,
        observed: FileIdentity,
    },
    RemoveOwned {
        old: OwnedFile,
        observed: FileIdentity,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpgradePlan {
    pub install_id: String,
    pub from_version: String,
    pub to_version: String,
    pub operations: Vec<FileOperation>,
    pub retained_conflicts: Vec<String>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DataSelection {
    pub configuration: bool,
    pub database: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UninstallPreview {
    pub preview_id: String,
    pub install_id: String,
    pub database_path: Option<PathBuf>,
    pub database_identity: Option<FileIdentity>,
    pub selection: DataSelection,
    pub application_files: Vec<OwnedFile>,
    pub configuration_files: Vec<PathBuf>,
    pub database_files: Vec<PathBuf>,
}

pub fn normalize_relative_path(path: &str) -> Result<String> {
    ensure!(
        !path.is_empty() && path.len() <= 240 && path.is_ascii(),
        "资源路径无效"
    );
    ensure!(
        !path.contains(['\\', ':']) && !path.starts_with('/'),
        "资源路径必须是规范相对路径"
    );
    for part in path.split('/') {
        validate_component(part)?;
    }
    Ok(path.to_ascii_lowercase())
}
fn validate_component(part: &str) -> Result<()> {
    ensure!(
        !part.is_empty() && part != "." && part != ".." && !part.ends_with([' ', '.']),
        "路径分段无效"
    );
    ensure!(
        !part
            .chars()
            .any(|c| c.is_control() || "<>:\"|?*".contains(c)),
        "路径字符无效"
    );
    let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
    ensure!(
        !["CON", "PRN", "AUX", "NUL", "CLOCK$", "CONIN$", "CONOUT$"].contains(&stem.as_str())
            && !(stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9')),
        "设备名称不能作为资源"
    );
    Ok(())
}
pub fn valid_sha256(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn validate_owned_files(files: &[OwnedFile]) -> Result<()> {
    ensure!(!files.is_empty() && files.len() <= 128, "资源数量无效");
    let mut names = BTreeSet::new();
    for file in files {
        let path = normalize_relative_path(&file.relative_path)?;
        ensure!(names.insert(path.clone()), "资源路径冲突");
        ensure!(
            file.size > 0 && file.size <= 1024 * 1024 * 1024 && valid_sha256(&file.sha256),
            "资源长度或校验无效"
        );
        let allowed = match file.role {
            FileRole::Application => path == "ww-studyapp.exe",
            FileRole::Uninstaller => path == "uninstall.exe",
            FileRole::Runtime => !path.contains('/') && path.ends_with(".dll"),
            FileRole::Notice => matches!(
                path.as_str(),
                "license"
                    | "third-party-notices.txt"
                    | "readme.md"
                    | "release-notes.md"
                    | "support/license"
                    | "support/third-party-notices.txt"
                    | "support/readme.md"
                    | "support/release-notes.md"
            ),
            FileRole::Manifest => path == "support/manifest.json",
        };
        ensure!(allowed, "资源类型与路径不匹配");
    }
    Ok(())
}
pub fn validate_manifest(manifest: &PackageManifest) -> Result<()> {
    ensure!(
        manifest.schema == 1
            && manifest.product == PRODUCT
            && manifest.architecture == ARCHITECTURE,
        "不支持的应用资源清单"
    );
    semver::Version::parse(&manifest.version)?;
    validate_owned_files(&manifest.files)
}
pub fn validate_receipt(receipt: &InstallReceipt) -> Result<()> {
    ensure!(
        receipt.schema == 1
            && !receipt.install_id.is_empty()
            && receipt.install_id.len() <= 100
            && receipt
                .install_id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-'),
        "安装记录无效"
    );
    absolute_components(&receipt.install_root)?;
    ensure!(
        receipt.root_identity.file_id != 0
            && receipt.root_identity.size == 0
            && receipt.root_identity.sha256.is_empty(),
        "安装目录身份无效"
    );
    ensure!(
        valid_sha256(&receipt.source_package_sha256)
            || (matches!(receipt.owner, InstallOwner::Msi { .. })
                && receipt.source_package_sha256.is_empty()),
        "安装来源校验无效"
    );
    semver::Version::parse(&receipt.version)?;
    if let InstallOwner::Msi {
        product_code,
        upgrade_code,
        user_sid,
    } = &receipt.owner
    {
        ensure!(
            valid_product_code(product_code)
                && upgrade_code.eq_ignore_ascii_case(MSI_UPGRADE_CODE)
                && valid_user_sid(user_sid),
            "MSI 安装身份无效"
        );
    }
    validate_owned_files(&receipt.files)
}
pub fn valid_user_sid(sid: &str) -> bool {
    sid.starts_with("S-1-")
        && sid.len() > 4
        && sid.len() <= 184
        && sid[4..]
            .split('-')
            .all(|part| !part.is_empty() && part.bytes().all(|c| c.is_ascii_digit()))
}
pub fn valid_product_code(code: &str) -> bool {
    code.len() == 38
        && code.starts_with('{')
        && code.ends_with('}')
        && uuid::Uuid::parse_str(&code[1..37]).is_ok()
}
/// Component comparison is independent of the host path parser. UNC/device paths
/// and dot aliases are deliberately unsupported for maintenance.
pub fn absolute_components(path: &Path) -> Result<Vec<String>> {
    let text = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("路径编码无效"))?;
    ensure!(
        !text.starts_with("\\\\") && !text.starts_with("//"),
        "不支持网络或设备路径"
    );
    let text = text.replace('\\', "/");
    let (prefix, rest) = if text.len() >= 3
        && text.as_bytes()[0].is_ascii_alphabetic()
        && text.as_bytes().get(1..3) == Some(b":/")
    {
        (text[..2].to_ascii_lowercase(), &text[3..])
    } else if cfg!(not(windows)) && text.starts_with('/') {
        ("/".to_owned(), &text[1..])
    } else {
        anyhow::bail!("维护路径必须是本地绝对路径");
    };
    let mut result = vec![prefix];
    if !rest.is_empty() {
        for part in rest.split('/') {
            validate_component(part)?;
            result.push(part.to_lowercase());
        }
    }
    Ok(result)
}
pub fn same_absolute_path(a: &Path, b: &Path) -> Result<bool> {
    Ok(absolute_components(a)? == absolute_components(b)?)
}
