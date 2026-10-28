//! Handle-bound single-file operations; callers retain directory guards.
use anyhow::{Context, Result, ensure};
use std::{
    fs::{File, OpenOptions},
    path::Path,
};
use studyapp_desktop::maintenance::FileIdentity;

pub(super) fn checked_handle(path: &Path, expected: &FileIdentity) -> Result<File> {
    let _guard = crate::storage_backend::plain_handle(path, false, false)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options
            .access_mode(0x8000_0000 | 0x0001_0000)
            .share_mode(1)
            .custom_flags(0x0020_0000);
    }
    let file = options.open(path).context("维护资源正在使用或不可访问")?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(
            file.metadata()?.file_attributes() & 0x400 == 0,
            "维护资源是重解析点"
        );
    }
    let (actual, links) = crate::maintenance_runtime::handle_identity(&file, false)?;
    ensure!(links == 1 && actual == *expected, "维护资源身份已改变");
    Ok(file)
}

pub(super) fn move_exact(source: &Path, destination: &Path, identity: &FileIdentity) -> Result<()> {
    let _source_guards =
        crate::storage_backend::guard_directories(source.parent().context("源目录无效")?, false)?;
    let _destination_guards = crate::storage_backend::guard_directories(
        destination.parent().context("目标目录无效")?,
        false,
    )?;
    let file = checked_handle(source, identity)?;
    ensure!(!destination.try_exists()?, "目标路径已存在");
    #[cfg(windows)]
    {
        use std::os::windows::{ffi::OsStrExt, io::AsRawHandle};
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_RENAME_INFO, FileRenameInfo, SetFileInformationByHandle,
        };
        // The guarded parent is resolved once to Windows' extended-length form;
        // relative fixture/user paths may otherwise exceed legacy MAX_PATH.
        let destination = std::fs::canonicalize(destination.parent().context("目标目录无效")?)?
            .join(destination.file_name().context("目标文件名无效")?);
        let name: Vec<u16> = destination.as_os_str().encode_wide().collect();
        let bytes = std::mem::offset_of!(FILE_RENAME_INFO, FileName) + (name.len() + 1) * 2;
        let mut storage = vec![0usize; bytes.div_ceil(std::mem::size_of::<usize>())];
        let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
        // SAFETY: aligned zeroed storage holds the header and entire UTF-16 name.
        unsafe {
            (*info).RootDirectory = std::ptr::null_mut();
            (*info).FileNameLength = (name.len() * 2).try_into()?;
            std::ptr::copy_nonoverlapping(name.as_ptr(), (*info).FileName.as_mut_ptr(), name.len());
            ensure!(
                SetFileInformationByHandle(
                    file.as_raw_handle(),
                    FileRenameInfo,
                    info.cast(),
                    bytes.try_into()?
                ) != 0,
                "无法移动已验证资源至 {}：{}",
                destination.display(),
                std::io::Error::last_os_error()
            );
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = file;
        anyhow::bail!("此维护操作仅支持 Windows")
    }
}
pub(super) fn delete_exact(path: &Path, identity: &FileIdentity) -> Result<()> {
    let _guards =
        crate::storage_backend::guard_directories(path.parent().context("文件目录无效")?, false)?;
    let file = checked_handle(path, identity)?;
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle,
        };
        let info = FILE_DISPOSITION_INFO { DeleteFile: true };
        // SAFETY: live DELETE-capable handle and exact native input layout.
        ensure!(
            unsafe {
                SetFileInformationByHandle(
                    file.as_raw_handle(),
                    FileDispositionInfo,
                    (&info as *const FILE_DISPOSITION_INFO).cast(),
                    std::mem::size_of_val(&info) as u32,
                )
            } != 0,
            "无法删除已验证资源：{}",
            std::io::Error::last_os_error()
        );
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = file;
        anyhow::bail!("此维护操作仅支持 Windows")
    }
}
pub(super) fn delete_empty_directory(path: &Path, identity: &FileIdentity) -> Result<()> {
    let _guards =
        crate::storage_backend::guard_directories(path.parent().context("目录父路径无效")?, false)?;
    #[cfg(windows)]
    {
        use std::os::windows::{
            fs::{MetadataExt, OpenOptionsExt},
            io::AsRawHandle,
        };
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle,
        };
        let file = OpenOptions::new()
            .read(true)
            .access_mode(0x8000_0000 | 0x10000)
            .share_mode(3)
            .custom_flags(0x0020_0000 | 0x0200_0000)
            .open(path)?;
        ensure!(
            file.metadata()?.file_attributes() & 0x400 == 0
                && crate::maintenance_runtime::handle_identity(&file, true)?.0 == *identity,
            "目录身份已改变"
        );
        ensure!(
            std::fs::read_dir(path)?.next().is_none(),
            "目录包含保留文件"
        );
        let info = FILE_DISPOSITION_INFO { DeleteFile: true };
        // SAFETY: DELETE-capable directory handle; Windows rejects nonempty directories atomically.
        ensure!(
            unsafe {
                SetFileInformationByHandle(
                    file.as_raw_handle(),
                    FileDispositionInfo,
                    (&info as *const FILE_DISPOSITION_INFO).cast(),
                    std::mem::size_of_val(&info) as u32,
                )
            } != 0,
            "无法移除空维护目录"
        );
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = identity;
        bail!("此维护操作仅支持 Windows")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> std::path::PathBuf {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .join(".scratch/installation-20261026/test-temp")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&root).unwrap();
        root
    }
    #[test]
    fn maintenance_handle_moves_and_deletes_only_captured_file() {
        let root = fixture();
        let source = root.join("owned.dll");
        let target = root.join("backup");
        std::fs::write(&source, b"owned").unwrap();
        let identity = crate::maintenance_runtime::observe_file(&source, "owned.dll")
            .unwrap()
            .identity;
        move_exact(&source, &target, &identity).unwrap();
        assert!(!source.exists());
        assert_eq!(std::fs::read(&target).unwrap(), b"owned");
        delete_exact(&target, &identity).unwrap();
        assert!(!target.exists());
    }
    #[test]
    fn maintenance_handle_refuses_changed_and_preexisting_targets() {
        let root = fixture();
        let source = root.join("owned.dll");
        let target = root.join("unknown");
        std::fs::write(&source, b"owned").unwrap();
        std::fs::write(&target, b"unknown").unwrap();
        let identity = crate::maintenance_runtime::observe_file(&source, "owned.dll")
            .unwrap()
            .identity;
        assert!(move_exact(&source, &target, &identity).is_err());
        std::fs::write(&source, b"changed").unwrap();
        assert!(delete_exact(&source, &identity).is_err());
        assert_eq!(std::fs::read(&source).unwrap(), b"changed");
        assert_eq!(std::fs::read(&target).unwrap(), b"unknown");
    }
}
