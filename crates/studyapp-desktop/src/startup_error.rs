//! Startup diagnostics contain paths and categories, never file contents.
use std::{
    fmt, io,
    path::{Path, PathBuf},
};

// SECURITY_MANDATORY_MEDIUM_RID from the Windows SDK. Below Medium, a process
// cannot normally write to the user's Medium-integrity AppData directory.
const MEDIUM_INTEGRITY_RID: u32 = 0x2000;

#[cfg(windows)]
pub(super) fn process_integrity_level() -> Option<u32> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::{
        Security::{
            GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, IsValidSid,
            SECURITY_MAX_SID_SIZE, TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TokenIntegrityLevel,
        },
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };

    let mut raw_token = std::ptr::null_mut();
    // Query only; neither the process token nor any filesystem permissions change.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw_token) } == 0 {
        return None;
    }
    // The successful API call transfers this real handle to us. OwnedHandle
    // closes it on every return path; GetCurrentProcess's pseudo-handle is not owned.
    let token = unsafe { OwnedHandle::from_raw_handle(raw_token) };
    const BUFFER_WORDS: usize = (size_of::<TOKEN_MANDATORY_LABEL>()
        + SECURITY_MAX_SID_SIZE as usize)
        .div_ceil(size_of::<usize>());
    // usize storage provides the pointer alignment required by TOKEN_MANDATORY_LABEL.
    let mut buffer = [0_usize; BUFFER_WORDS];
    let buffer_bytes = size_of_val(&buffer) as u32;
    let mut returned = 0_u32;
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenIntegrityLevel,
            buffer.as_mut_ptr().cast(),
            buffer_bytes,
            &mut returned,
        )
    } == 0
        || returned < size_of::<TOKEN_MANDATORY_LABEL>() as u32
        || returned > buffer_bytes
    {
        return None;
    }
    // A successful query initializes the label and its SID within the live buffer.
    let label = unsafe { &*buffer.as_ptr().cast::<TOKEN_MANDATORY_LABEL>() };
    let sid = label.Label.Sid;
    let start = buffer.as_ptr() as usize;
    let end = start.checked_add(returned as usize)?;
    let sid_start = sid as usize;
    if sid.is_null() || sid_start < start || sid_start.checked_add(8)? > end {
        return None;
    }
    // The checked SID header contains the subauthority count; bound the trailing
    // data before asking Windows to validate or read a subauthority.
    let count = unsafe { *GetSidSubAuthorityCount(sid) };
    if count == 0
        || sid_start.checked_add(8 + usize::from(count) * size_of::<u32>())? > end
        || unsafe { IsValidSid(sid) } == 0
    {
        return None;
    }
    Some(unsafe { *GetSidSubAuthority(sid, u32::from(count - 1)) })
}

#[cfg(not(windows))]
fn process_integrity_level() -> Option<u32> {
    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartupStage {
    PrepareDirectory,
    LibraryLock,
    Database,
    Preferences,
    RememberLocation,
}

impl StartupStage {
    pub fn label(self) -> &'static str {
        match self {
            Self::PrepareDirectory => "访问资料库目录",
            Self::LibraryLock => "取得资料库使用权限",
            Self::Database => "打开学习资料库",
            Self::Preferences => "读取资料库设置",
            Self::RememberLocation => "保存资料库位置",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartupFailureKind {
    PermissionDenied,
    RestrictedProcess,
    InUse,
    InvalidLocation,
    InvalidConfiguration,
    DatabaseUnavailable,
    Unavailable,
}

#[derive(Clone, Debug)]
pub struct StartupFailure {
    pub database: PathBuf,
    pub path: PathBuf,
    pub kind: StartupFailureKind,
    pub stage: StartupStage,
    pub message: String,
    pub action: &'static str,
}

impl StartupFailure {
    pub(crate) fn from_error(
        database: &Path,
        stage: StartupStage,
        path: &Path,
        error: &anyhow::Error,
    ) -> Self {
        let permission_denied = error
            .chain()
            .find_map(|cause| cause.downcast_ref::<io::Error>())
            .is_some_and(|error| error.kind() == io::ErrorKind::PermissionDenied);
        let integrity = permission_denied.then(process_integrity_level).flatten();
        Self::from_error_with_integrity(database, stage, path, error, integrity)
    }

    fn from_error_with_integrity(
        database: &Path,
        stage: StartupStage,
        path: &Path,
        error: &anyhow::Error,
        process_integrity: Option<u32>,
    ) -> Self {
        let path = error
            .downcast_ref::<crate::storage_backend::StorageAccessPath>()
            .map_or(path, |context| context.0.as_path());
        let io = error
            .chain()
            .find_map(|cause| cause.downcast_ref::<io::Error>());
        let os_code = io.and_then(io::Error::raw_os_error);
        let kind = match io.map(io::Error::kind) {
            _ if cfg!(windows) && matches!(os_code, Some(32 | 33)) => StartupFailureKind::InUse,
            Some(io::ErrorKind::PermissionDenied)
                if process_integrity.is_some_and(|level| level < MEDIUM_INTEGRITY_RID) =>
            {
                StartupFailureKind::RestrictedProcess
            }
            Some(io::ErrorKind::PermissionDenied) => StartupFailureKind::PermissionDenied,
            Some(io::ErrorKind::WouldBlock) => StartupFailureKind::InUse,
            Some(
                io::ErrorKind::NotFound
                | io::ErrorKind::NotADirectory
                | io::ErrorKind::IsADirectory
                | io::ErrorKind::AlreadyExists
                | io::ErrorKind::InvalidInput,
            ) => StartupFailureKind::InvalidLocation,
            Some(_) => StartupFailureKind::Unavailable,
            None if stage == StartupStage::Preferences => StartupFailureKind::InvalidConfiguration,
            None if stage == StartupStage::Database => StartupFailureKind::DatabaseUnavailable,
            None => StartupFailureKind::Unavailable,
        };
        Self::new(database, stage, path, kind)
    }

    pub(crate) fn in_use(database: &Path, path: &Path) -> Self {
        Self::new(
            database,
            StartupStage::LibraryLock,
            path,
            StartupFailureKind::InUse,
        )
    }

    fn new(database: &Path, stage: StartupStage, path: &Path, kind: StartupFailureKind) -> Self {
        let (message, action) = match kind {
            StartupFailureKind::RestrictedProcess => (
                "当前程序运行在受限环境，无法写入所选资料库。",
                "请从普通用户终端将程序构建到受限工作区之外，或把发行包解压到普通目录运行，再重试。原资料已保留。",
            ),
            StartupFailureKind::PermissionDenied => (
                "当前账户没有足够权限访问资料库或其设置。",
                "请确认该位置可读写，并从正常的用户桌面启动应用后重试；也可以选择其他资料库目录。",
            ),
            StartupFailureKind::InUse => (
                "资料库或其文件正在被另一个程序使用。",
                "请关闭使用此资料库的其他 WW-StudyApp 窗口或占用文件的程序后重试，也可以选择其他资料库目录。",
            ),
            StartupFailureKind::InvalidLocation => (
                "资料库位置不存在或路径类型不正确。",
                "请检查磁盘是否已连接、路径是否指向普通目录，然后重试或选择其他资料库目录。",
            ),
            StartupFailureKind::InvalidConfiguration => (
                "资料库设置无法读取或验证。",
                "原设置文件已保留。请检查设置文件是否完整后重试，或选择其他资料库目录。",
            ),
            StartupFailureKind::DatabaseUnavailable => (
                "学习资料库暂时无法打开或验证。",
                "请检查磁盘、文件权限及其他程序的占用后重试；如需恢复，请先保留原资料库并使用已有备份。",
            ),
            StartupFailureKind::Unavailable => (
                "资料库暂时无法访问。",
                "请检查磁盘是否可用、文件是否被占用后重试，也可以选择其他资料库目录。",
            ),
        };
        Self {
            database: database.to_owned(),
            path: path.to_owned(),
            kind,
            stage,
            message: message.into(),
            action,
        }
    }
}

impl fmt::Display for StartupFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}{}", self.message, self.action)
    }
}

impl std::error::Error for StartupFailure {}

#[cfg(test)]
mod tests {
    use super::{StartupFailure, StartupFailureKind, StartupStage};
    use std::{io, path::Path};

    fn diagnose(kind: io::ErrorKind, integrity: Option<u32>) -> StartupFailure {
        let database = Path::new("library/studyapp.sqlite3");
        StartupFailure::from_error_with_integrity(
            database,
            StartupStage::LibraryLock,
            &database.with_extension("lock"),
            &io::Error::new(kind, "private OS detail").into(),
            integrity,
        )
    }

    #[test]
    fn permission_denied_in_low_integrity_process_explains_build_location() {
        for integrity in [0, 0x1000, 0x1fff] {
            let failure = diagnose(io::ErrorKind::PermissionDenied, Some(integrity));
            assert_eq!(failure.kind, StartupFailureKind::RestrictedProcess);
            assert!(failure.message.contains("受限环境"));
            assert!(failure.action.contains("构建到受限工作区之外"));
            assert!(failure.action.contains("发行包解压到普通目录"));
            assert!(failure.action.contains("原资料已保留"));
            assert!(!failure.to_string().contains("private OS"));
            assert_eq!(failure.path, Path::new("library/studyapp.lock"));
        }
    }

    #[test]
    fn normal_or_unknown_integrity_preserves_permission_diagnosis() {
        for integrity in [None, Some(0x2000), Some(0x3000), Some(0x4000)] {
            let failure = diagnose(io::ErrorKind::PermissionDenied, integrity);
            assert_eq!(failure.kind, StartupFailureKind::PermissionDenied);
            assert!(failure.message.contains("权限"));
            assert!(!failure.to_string().contains("受限工作区"));
        }
    }

    #[test]
    fn low_integrity_does_not_override_unrelated_filesystem_failures() {
        assert_eq!(
            diagnose(io::ErrorKind::WouldBlock, Some(0x1000)).kind,
            StartupFailureKind::InUse
        );
        assert_eq!(
            diagnose(io::ErrorKind::NotFound, Some(0x1000)).kind,
            StartupFailureKind::InvalidLocation
        );
    }

    #[cfg(windows)]
    #[test]
    fn current_token_integrity_is_readable_and_used_for_permission_diagnosis() {
        let integrity =
            super::process_integrity_level().expect("current process token is queryable");
        let expected = if integrity < super::MEDIUM_INTEGRITY_RID {
            StartupFailureKind::RestrictedProcess
        } else {
            StartupFailureKind::PermissionDenied
        };
        let database = Path::new("library/studyapp.sqlite3");
        let failure = StartupFailure::from_error(
            database,
            StartupStage::LibraryLock,
            &database.with_extension("lock"),
            &io::Error::from_raw_os_error(5).into(),
        );
        assert_eq!(failure.kind, expected);
    }
}
