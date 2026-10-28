//! Bounded, conditional synchronization of UUID-named Markdown notes.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;

mod local;
mod object_store;
mod planner;
mod webdav;
pub use local::LocalFolderProvider;
pub use object_store::{ObjectAddressing, ObjectStoreConfig, ObjectStoreProvider};
pub use planner::plan_sync;
pub use webdav::WebDavProvider;

pub const MAX_CONTENT_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_XML_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 10_000;
pub const MAX_SYNC_BYTES: usize = 64 * 1024 * 1024;
pub type Result<T> = std::result::Result<T, SyncError>;

#[derive(Debug)]
pub enum SyncError {
    InvalidId,
    InvalidPath,
    InvalidUrl,
    InvalidContent,
    LimitExceeded,
    Conflict,
    MissingRevision,
    InvalidXml,
    Http(u16),
    Transport,
    Io(std::io::ErrorKind),
    LockBusy,
}
impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingRevision => write!(f, "WebDAV 未提供强 ETag，已拒绝覆盖现有笔记"),
            Self::Conflict => write!(f, "远端内容已更改，请重新同步；未覆盖未知版本"),
            Self::LockBusy => write!(f, "同步目录正在被其他 StudyApp 使用，请稍后重试"),
            Self::Http(401 | 403) => write!(f, "云端拒绝访问，请检查账户密钥、存储桶及读写权限"),
            Self::Http(301..=399) => write!(
                f,
                "云端地址发生重定向，请填写服务商提供的最终 Endpoint 与正确 Region"
            ),
            Self::Http(404) => write!(f, "云端路径或存储桶不存在，请检查目标配置"),
            Self::Http(code) => write!(f, "云端请求失败（HTTP {code}），请检查服务状态后重试"),
            Self::Transport => write!(f, "云端连接失败，请检查网络、证书、代理及 Endpoint 后重试"),
            Self::InvalidUrl => {
                write!(f, "云端地址无效，请填写不含账户、查询参数的 HTTPS 服务地址")
            }
            Self::InvalidPath => write!(f, "云端目录、存储桶或对象路径无效，请检查目标配置"),
            Self::Io(std::io::ErrorKind::NotFound) => {
                write!(f, "同步目录或文件不存在，请重新选择位置")
            }
            Self::Io(std::io::ErrorKind::PermissionDenied) => {
                write!(f, "无法访问同步目录，请检查权限或文件占用")
            }
            _ => write!(f, "同步失败：{self:?}"),
        }
    }
}
impl std::error::Error for SyncError {}
impl From<std::io::Error> for SyncError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.kind())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoteNote {
    pub content: String,
    pub hash: String,
    pub revision: Option<String>,
}

/// `None` means create-only. Existing objects require a previously read revision.
/// Implementations deliberately expose no deletion operation.
pub trait Provider: Send + Sync {
    fn list(&self) -> Result<Vec<String>>;
    fn read(&self, id: &str) -> Result<Option<RemoteNote>>;
    fn write(&self, id: &str, content: &str, expected_revision: Option<&str>)
    -> Result<RemoteNote>;
    fn test_connection(&self) -> Result<()>;
    /// Immutable assets have no overwrite or deletion operation and never enter note listings.
    fn read_asset(&self, key: &str) -> Result<Option<Vec<u8>>>;
    fn write_asset(&self, key: &str, bytes: &[u8]) -> Result<()>;
}

/// Image-only storage; implementations never need note revisions or deletion.
pub trait AssetProvider: Send + Sync {
    fn get_asset(&self, key: &str) -> Result<Option<Vec<u8>>>;
    fn put_asset(&self, key: &str, bytes: &[u8]) -> Result<()>;
    fn check_connection(&self) -> Result<()>;
    fn identity(&self) -> Option<String> {
        None
    }
}

impl<T: Provider> AssetProvider for T {
    fn get_asset(&self, key: &str) -> Result<Option<Vec<u8>>> {
        Provider::read_asset(self, key)
    }
    fn put_asset(&self, key: &str, bytes: &[u8]) -> Result<()> {
        Provider::write_asset(self, key, bytes)
    }
    fn check_connection(&self) -> Result<()> {
        Provider::test_connection(self)
    }
}

pub struct NoteAssets {
    provider: Box<dyn Provider>,
    identity: String,
}
impl NoteAssets {
    pub fn new(provider: Box<dyn Provider>, identity: String) -> Self {
        Self { provider, identity }
    }
}
impl AssetProvider for NoteAssets {
    fn get_asset(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.provider.read_asset(key)
    }
    fn put_asset(&self, key: &str, bytes: &[u8]) -> Result<()> {
        self.provider.write_asset(key, bytes)
    }
    fn check_connection(&self) -> Result<()> {
        self.provider.test_connection()
    }
    fn identity(&self) -> Option<String> {
        Some(self.identity.clone())
    }
}

pub fn validate_asset_key(key: &str) -> Result<()> {
    let parts: Vec<_> = key.split('/').collect();
    if parts.len() != 3 || parts[0] != "assets" {
        return Err(SyncError::InvalidPath);
    }
    validate_id(parts[1])?;
    let name = parts[2];
    if name.len() > 240
        || name.is_empty()
        || name.starts_with('.')
        || !name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(SyncError::InvalidPath);
    }
    if !["png", "jpg", "jpeg", "gif", "webp", "bmp"]
        .contains(&name.rsplit('.').next().unwrap_or(""))
    {
        return Err(SyncError::InvalidPath);
    }
    Ok(())
}

fn bounded_bytes(reader: impl Read) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_CONTENT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_CONTENT_BYTES {
        return Err(SyncError::LimitExceeded);
    }
    Ok(bytes)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncNote {
    pub id: String,
    pub title: String,
    pub content: String,
    pub hash: String,
    pub managed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Baseline {
    pub local_hash: String,
    pub remote_hash: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NotePlan {
    pub id: String,
    pub expected_local_hash: Option<String>,
    pub action: SyncAction,
    pub next_baseline: Baseline,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SyncAction {
    Unchanged,
    Push {
        content: String,
        expected_revision: Option<String>,
    },
    Pull {
        content: String,
    },
    Conflict {
        local_content: String,
        remote_content: String,
    },
    ImportRemote {
        content: String,
    },
    CopyExternal {
        content: String,
    },
}
pub fn content_hash(content: &str) -> String {
    content_hash_bytes(content.as_bytes())
}
pub fn content_hash_bytes(content: &[u8]) -> String {
    format!("{:x}", Sha256::digest(content))
}

fn validate_id(id: &str) -> Result<()> {
    let uuid = uuid::Uuid::parse_str(id).map_err(|_| SyncError::InvalidId)?;
    if uuid.to_string() != id {
        return Err(SyncError::InvalidId);
    }
    Ok(())
}
fn validate_content(content: &str) -> Result<()> {
    if content.len() > MAX_CONTENT_BYTES {
        return Err(SyncError::LimitExceeded);
    }
    if content.contains('\0') {
        return Err(SyncError::InvalidContent);
    }
    Ok(())
}
fn bounded_read(reader: impl Read, limit: usize) -> Result<String> {
    let mut bytes = Vec::new();
    reader.take((limit + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(SyncError::LimitExceeded);
    }
    String::from_utf8(bytes).map_err(|_| SyncError::InvalidContent)
}

#[cfg(test)]
mod tests;
