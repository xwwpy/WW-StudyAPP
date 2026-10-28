//! Future service boundaries. No remote service or credentials are configured by the local edition.
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum StorageMode {
    Local,
    Cloud,
    Hybrid,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuthSession {
    pub account_id: String,
    pub expires_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncConflict {
    pub entity_id: String,
    pub local_revision: u64,
    pub remote_revision: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncResult {
    pub uploaded: usize,
    pub downloaded: usize,
    pub conflicts: Vec<SyncConflict>,
}
pub trait AuthProvider {
    fn begin_wechat_authorization(&self) -> Result<String>;
    fn poll_authorization(&self, authorization_id: &str) -> Result<Option<AuthSession>>;
    fn restore_session(&self) -> Result<Option<AuthSession>>;
    fn sign_out(&self) -> Result<()>;
}
pub trait SyncProvider {
    fn synchronize(&self, mode: StorageMode) -> Result<SyncResult>;
}
#[derive(Clone, Debug, Default)]
pub struct UnavailableServices;
impl AuthProvider for UnavailableServices {
    fn begin_wechat_authorization(&self) -> Result<String> {
        bail!("微信登录尚未连接服务")
    }
    fn poll_authorization(&self, _: &str) -> Result<Option<AuthSession>> {
        bail!("微信登录尚未连接服务")
    }
    fn restore_session(&self) -> Result<Option<AuthSession>> {
        bail!("微信登录尚未连接服务")
    }
    fn sign_out(&self) -> Result<()> {
        bail!("微信登录尚未连接服务")
    }
}
impl SyncProvider for UnavailableServices {
    fn synchronize(&self, _: StorageMode) -> Result<SyncResult> {
        bail!("云端同步尚未配置，本地资料已保存在此设备")
    }
}
