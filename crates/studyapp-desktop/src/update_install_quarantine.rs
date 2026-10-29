//! Preserve existing macOS quarantine metadata without inventing provenance.
use std::io;

#[cfg(target_os = "macos")]
use std::{ffi::CString, os::unix::ffi::OsStrExt, path::Path};

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn getxattr(
        path: *const std::ffi::c_char,
        name: *const std::ffi::c_char,
        value: *mut std::ffi::c_void,
        size: usize,
        position: u32,
        options: i32,
    ) -> isize;
    fn setxattr(
        path: *const std::ffi::c_char,
        name: *const std::ffi::c_char,
        value: *const std::ffi::c_void,
        size: usize,
        position: u32,
        options: i32,
    ) -> i32;
}

#[cfg(target_os = "macos")]
pub(crate) fn read(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let path = CString::new(path.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    let mut bytes = vec![0u8; 65536];
    // XATTR_NOFOLLOW: do not follow a substituted staging symlink.
    let length = unsafe {
        getxattr(
            path.as_ptr(),
            c"com.apple.quarantine".as_ptr(),
            bytes.as_mut_ptr().cast(),
            bytes.len(),
            0,
            1,
        )
    };
    if length < 0 {
        return absent_or_error(io::Error::last_os_error());
    }
    bytes.truncate(length as usize);
    Ok(Some(bytes))
}

#[cfg(target_os = "macos")]
pub(crate) fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let native = CString::new(path.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    if unsafe {
        setxattr(
            native.as_ptr(),
            c"com.apple.quarantine".as_ptr(),
            bytes.as_ptr().cast(),
            bytes.len(),
            0,
            1,
        )
    } != 0
    {
        return Err(format!(
            "Cannot preserve update quarantine: {}",
            io::Error::last_os_error()
        ));
    }
    if read(path)?.as_deref() != Some(bytes) {
        return Err("Update quarantine changed during propagation".into());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub(super) fn propagate(source: &Path, target: &Path) -> Result<(), String> {
    if let Some(value) = read(source)? {
        if read(target)?.is_some_and(|existing| existing != value) {
            return Err("Conflicting existing quarantine metadata; refusing to replace it".into());
        }
        write(target, &value)?;
    }
    Ok(())
}

fn absent_or_error(error: io::Error) -> Result<Option<Vec<u8>>, String> {
    if error.raw_os_error() == Some(93) {
        // Darwin ENOATTR only.
        Ok(None)
    } else {
        Err(format!("Cannot read update quarantine: {error}"))
    }
}

#[cfg(target_os = "macos")]
pub(super) fn preserve_tree(source: &Path, target: &Path) -> Result<(), String> {
    let source_meta = std::fs::symlink_metadata(source).map_err(|e| e.to_string())?;
    let target_meta = std::fs::symlink_metadata(target).map_err(|e| e.to_string())?;
    if source_meta.file_type() != target_meta.file_type() {
        return Err("Quarantine propagation target type changed".into());
    }
    propagate(source, target)?;
    if source_meta.is_dir() {
        for entry in std::fs::read_dir(source).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            preserve_tree(&entry.path(), &target.join(entry.file_name()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn update_quarantine_only_enoattr_means_absent() {
        assert_eq!(absent_or_error(io::Error::from_raw_os_error(93)), Ok(None));
        for code in [1, 2, 5, 13, 34] {
            assert!(absent_or_error(io::Error::from_raw_os_error(code)).is_err());
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn update_quarantine_preserves_presence_absence_and_errors() {
        let root =
            std::env::temp_dir().join(format!("studyapp-quarantine-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let source = root.join("source");
        let target = root.join("target");
        std::fs::write(&source, b"image").unwrap();
        std::fs::write(&target, b"bundle").unwrap();
        assert_eq!(read(&source).unwrap(), None);
        let existing = b"0083;1234;ExistingSource;existing\n";
        write(&target, existing).unwrap();
        propagate(&source, &target).unwrap();
        assert_eq!(read(&target).unwrap().as_deref(), Some(existing.as_slice()));
        write(&source, existing).unwrap();
        propagate(&source, &target).unwrap();
        assert_eq!(read(&target).unwrap(), read(&source).unwrap());
        assert!(read(&root.join("missing")).is_err());
        assert!(propagate(&root.join("missing"), &target).is_err());
        assert!(propagate(&source, &root.join("missing")).is_err());
        write(&source, b"0083;1234;OtherSource;other").unwrap();
        assert!(propagate(&source, &target).is_err());
        assert_eq!(read(&target).unwrap().as_deref(), Some(existing.as_slice()));
        std::fs::remove_file(&source).unwrap();
        std::fs::remove_file(&target).unwrap();
        std::fs::remove_dir(&root).unwrap();
    }
}
