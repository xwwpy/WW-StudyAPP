//! Atomic claim in an owner-controlled directory; never unlink a shared source path.
use super::*;
use std::{
    ffi::{CString, c_char, c_int, c_uint},
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
        io::{AsRawFd, FromRawFd},
    },
};
#[cfg(target_os = "linux")]
const DIRECTORY: c_int = 0x10000;
#[cfg(target_os = "linux")]
const CLOEXEC: c_int = 0x80000;
#[cfg(target_os = "linux")]
const NONBLOCK: c_int = 0x800;
#[cfg(target_os = "linux")]
const NOFOLLOW: c_int = 0x20000;
#[cfg(target_os = "macos")]
const DIRECTORY: c_int = 0x100000;
#[cfg(target_os = "macos")]
const CLOEXEC: c_int = 0x1000000;
#[cfg(target_os = "macos")]
const NONBLOCK: c_int = 0x4;
#[cfg(target_os = "macos")]
const NOFOLLOW: c_int = 0x100;
unsafe extern "C" {
    fn geteuid() -> c_uint;
    fn openat(fd: c_int, path: *const c_char, flags: c_int, ...) -> c_int;
    fn mkdirat(fd: c_int, path: *const c_char, mode: c_uint) -> c_int;
    fn renameat(oldfd: c_int, old: *const c_char, newfd: c_int, new: *const c_char) -> c_int;
    fn linkat(
        oldfd: c_int,
        old: *const c_char,
        newfd: c_int,
        new: *const c_char,
        flags: c_int,
    ) -> c_int;
    fn unlinkat(fd: c_int, path: *const c_char, flags: c_int) -> c_int;
}
fn name(path: &std::ffi::OsStr) -> Result<CString> {
    CString::new(path.as_bytes()).map_err(|_| SyncError::InvalidPath)
}
fn syscall(value: c_int) -> Result<()> {
    if value < 0 {
        Err(std::io::Error::last_os_error().into())
    } else {
        Ok(())
    }
}
fn open_at(parent: &File, part: &std::ffi::OsStr, directory: bool) -> Result<File> {
    let part = name(part)?;
    let fd = unsafe {
        openat(
            parent.as_raw_fd(),
            part.as_ptr(),
            NOFOLLOW | CLOEXEC | NONBLOCK | if directory { DIRECTORY } else { 0 },
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn owned(dir: &File) -> Result<()> {
    let m = dir.metadata()?;
    if !m.is_dir() || m.uid() != unsafe { geteuid() } || m.mode() & 0o022 != 0 {
        return Err(SyncError::UnsupportedRemoval(
            "Unix removal requires an owned directory with no group/other writers",
        ));
    }
    Ok(())
}
// Pin each component from the selected owned root; no symlink traversal.
fn parent(provider: &LocalFolderProvider, path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).custom_flags(DIRECTORY | NOFOLLOW);
    let mut dir = options.open(&provider.root)?;
    owned(&dir)?;
    let relative = path
        .parent()
        .ok_or(SyncError::InvalidPath)?
        .strip_prefix(&provider.root)
        .map_err(|_| SyncError::InvalidPath)?;
    for part in relative.components() {
        let std::path::Component::Normal(part) = part else {
            return Err(SyncError::InvalidPath);
        };
        dir = open_at(&dir, part, true)?;
        owned(&dir)?;
    }
    Ok(dir)
}
fn inspect_locked(
    provider: &LocalFolderProvider,
    target: &str,
    asset: bool,
) -> Result<RemoteRemoval> {
    let path = provider.removal_path(target, asset)?;
    let dir = parent(provider, &path)?;
    let file = open_at(&dir, path.file_name().ok_or(SyncError::InvalidPath)?, false)?;
    if !file.metadata()?.is_file() {
        return Err(SyncError::InvalidPath);
    }
    let retained = provider.root.join("StudyApp/notes/.retained");
    if retained.exists() {
        let sentinel = retained.join("sentinel");
        parent(provider, &sentinel)?;
    }
    let metadata = file.metadata()?;
    if metadata.uid() != unsafe { geteuid() } || metadata.mode() & 0o022 != 0 {
        return Err(SyncError::UnsupportedRemoval(
            "Unix file must be owned and not group/other writable",
        ));
    }
    let plan = provider.inspect_removal_file(target, asset, &file)?;
    if file.metadata()?.nlink() != 1 + plan.confirmed_retained.len() as u64 {
        return Err(SyncError::UnsupportedRemoval(
            "Unix object has external or unproven hard links; cannot exclude external writers",
        ));
    }
    Ok(plan)
}
pub(super) fn inspect(
    provider: &LocalFolderProvider,
    target: &str,
    asset: bool,
) -> Result<RemoteRemoval> {
    let (_notes, _lock) = provider.locked_dir()?;
    inspect_locked(provider, target, asset)
}
// On mismatch restore create-only. If another file owns the original name, preserve
// the claimed object in its private transaction directory and report its exact path.
fn restore(
    dir: &File,
    original: &CString,
    txn: &File,
    claimed: &CString,
    recovery: &Path,
) -> Result<()> {
    if unsafe {
        linkat(
            txn.as_raw_fd(),
            claimed.as_ptr(),
            dir.as_raw_fd(),
            original.as_ptr(),
            0,
        )
    } < 0
    {
        return Err(SyncError::RemovalRecovery(
            recovery.to_string_lossy().into_owned(),
        ));
    }
    syscall(unsafe { unlinkat(txn.as_raw_fd(), claimed.as_ptr(), 0) })
}
/// Atomically claim a single directory entry under an already-pinned parent.
pub fn claim_unlink(
    dir: &File,
    path: &Path,
    revision: &str,
    hash: &str,
    bytes: u64,
    links: Option<u64>,
) -> Result<()> {
    owned(dir)?;
    let original = name(path.file_name().ok_or(SyncError::InvalidPath)?)?;
    let transaction = format!(".studyapp-removal-{}", uuid::Uuid::new_v4());
    let txn_name = name(std::ffi::OsStr::new(&transaction))?;
    syscall(unsafe { mkdirat(dir.as_raw_fd(), txn_name.as_ptr(), 0o700) })?;
    let txn = open_at(dir, std::ffi::OsStr::new(&transaction), true)?;
    owned(&txn)?;
    if txn.metadata()?.mode() & 0o777 != 0o700 {
        return Err(SyncError::UnsupportedRemoval(
            "Unix transaction directory must have exact 0700 permissions",
        ));
    }
    let recovery = path
        .parent()
        .ok_or(SyncError::InvalidPath)?
        .join(&transaction)
        .join("claimed");
    let claimed = CString::new("claimed").unwrap();
    #[cfg(test)]
    BEFORE_REMOVE.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook(path);
        }
    });
    syscall(unsafe {
        renameat(
            dir.as_raw_fd(),
            original.as_ptr(),
            txn.as_raw_fd(),
            claimed.as_ptr(),
        )
    })?;
    #[cfg(test)]
    AFTER_CLAIM.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook(path);
        }
    });
    let verify = (|| {
        let file = open_at(&txn, std::ffi::OsStr::new("claimed"), false)?;
        let before = file.metadata()?;
        if !before.is_file()
            || links.is_some_and(|links| before.nlink() != links)
            || before.len() != bytes
            || before.uid() != unsafe { geteuid() }
            || before.mode() & 0o022 != 0
        {
            return Err(SyncError::Conflict);
        }
        let identity = removal_identity(&file)?;
        let (actual, len) = removal_hash(&file, bytes)?;
        let after = file.metadata()?;
        if format!("{identity}:{actual}") != revision
            || actual != hash
            || len != bytes
            || before.mtime() != after.mtime()
            || before.mtime_nsec() != after.mtime_nsec()
            || before.ctime() != after.ctime()
            || before.ctime_nsec() != after.ctime_nsec()
        {
            return Err(SyncError::Conflict);
        }
        Ok(())
    })();
    if let Err(error) = verify {
        restore(dir, &original, &txn, &claimed, &recovery)?;
        return Err(error);
    }
    // Only the verified name inside the new private directory is unlinked.
    if let Err(error) = syscall(unsafe { unlinkat(txn.as_raw_fd(), claimed.as_ptr(), 0) }) {
        restore(dir, &original, &txn, &claimed, &recovery)?;
        return Err(error);
    }
    Ok(())
}
pub(super) fn execute(
    provider: &LocalFolderProvider,
    plan: &RemoteRemoval,
    asset: bool,
) -> Result<RemovalOutcome> {
    let (_notes, _lock) = provider.locked_dir()?;
    let mut current = inspect_locked(provider, &plan.target, asset)?;
    if plan.confirmed_retained.iter().enumerate().any(|(i, e)| {
        !current.confirmed_retained.contains(e) || plan.confirmed_retained[..i].contains(e)
    }) {
        return Err(SyncError::Conflict);
    }
    let mut links = 1 + current.confirmed_retained.len() as u64;
    let mut expected = plan.clone();
    current.confirmed_retained.clear();
    expected.confirmed_retained.clear();
    if current != expected {
        return Err(SyncError::Conflict);
    }
    let mut outcome = RemovalOutcome {
        state: RemovalState::VerifiedAbsent,
        residual: RemovalResidual::RetainedCopies,
        removed_retained: 0,
        remaining_confirmed: Vec::new(),
        recovery_paths: Vec::new(),
    };
    let mut entries = plan
        .confirmed_retained
        .iter()
        .map(|e| {
            (
                provider.root.join("StudyApp/notes/.retained").join(&e.name),
                e.revision.clone(),
                e.hash.clone(),
                e.bytes,
            )
        })
        .collect::<Vec<_>>();
    entries.push((
        provider.removal_path(&plan.target, asset)?,
        plan.revision.clone(),
        plan.hash.clone(),
        plan.bytes,
    ));
    for (index, (path, revision, hash, bytes)) in entries.iter().enumerate() {
        let removed = parent(provider, path)
            .and_then(|dir| claim_unlink(&dir, path, revision, hash, *bytes, Some(links)));
        if let Err(error) = removed {
            if outcome.removed_retained == 0 {
                return Err(error);
            }
            if let SyncError::RemovalRecovery(path) = error {
                outcome.recovery_paths.push(path);
            }
            outcome.state = RemovalState::Partial;
            outcome.remaining_confirmed =
                plan.confirmed_retained[index.min(plan.confirmed_retained.len())..].to_vec();
            return Ok(outcome);
        }
        links -= 1;
        if index < plan.confirmed_retained.len() {
            outcome.removed_retained += 1;
        }
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    const ID: &str = "123e4567-e89b-42d3-a456-426614174000";
    fn fixture() -> (PathBuf, LocalFolderProvider) {
        let root = tempfile::tempdir().unwrap().keep();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let p = LocalFolderProvider::new(&root).unwrap();
        p.write(ID, "note", None).unwrap();
        (root, p)
    }
    #[test]
    fn removal_unix_owned_note_and_asset_remove_only_confirmed_entries() {
        let (root, p) = fixture();
        let plan = p.inspect_note(ID).unwrap();
        assert_eq!(plan.confirmed_retained.len(), 1);
        let result = p.delete_note(&plan).unwrap();
        assert_eq!(result.state, RemovalState::VerifiedAbsent);
        assert_eq!(result.removed_retained, 1);
        assert!(p.read(ID).unwrap().is_none());
        let key = format!("assets/{ID}/image.png");
        p.write_asset(&key, b"image").unwrap();
        let plan = Provider::inspect_asset(&p, &key).unwrap();
        assert_eq!(
            Provider::delete_asset(&p, &plan).unwrap().removed_retained,
            1
        );
        assert!(p.read_asset(&key).unwrap().is_none());
        assert!(root.exists());
    }
    #[test]
    fn removal_unix_path_replacement_is_restored_without_unlinking_replacement() {
        let (root, p) = fixture();
        let mut plan = p.inspect_note(ID).unwrap();
        plan.confirmed_retained.clear();
        BEFORE_REMOVE.with(|h| {
            *h.borrow_mut() = Some(Box::new(|path| {
                fs::rename(path, path.with_extension("saved")).unwrap();
                fs::write(path, "replacement").unwrap();
            }))
        });
        assert!(matches!(p.delete_note(&plan), Err(SyncError::Conflict)));
        assert_eq!(
            fs::read_to_string(root.join(format!("StudyApp/notes/{ID}.md"))).unwrap(),
            "replacement"
        );
    }
    #[test]
    fn removal_unix_restore_collision_preserves_both_files_with_recovery_path() {
        let (root, p) = fixture();
        let mut plan = p.inspect_note(ID).unwrap();
        plan.confirmed_retained.clear();
        BEFORE_REMOVE.with(|h| {
            *h.borrow_mut() = Some(Box::new(|path| {
                fs::rename(path, path.with_extension("saved")).unwrap();
                fs::write(path, "claimed stranger").unwrap();
            }))
        });
        AFTER_CLAIM.with(|h| {
            *h.borrow_mut() = Some(Box::new(|path| fs::write(path, "new owner").unwrap()))
        });
        let Err(SyncError::RemovalRecovery(recovery)) = p.delete_note(&plan) else {
            panic!("must expose preserved claim");
        };
        assert_eq!(fs::read_to_string(recovery).unwrap(), "claimed stranger");
        assert_eq!(
            fs::read_to_string(root.join(format!("StudyApp/notes/{ID}.md"))).unwrap(),
            "new owner"
        );
    }
    #[test]
    fn exact_claim_handles_large_note_and_restores_replacement_before_claim() {
        let (root, _) = fixture();
        let dir = File::open(&root).unwrap();
        let path = root.join("large.md");
        let data = vec![b'x'; MAX_CONTENT_BYTES + 4096];
        fs::write(&path, &data).unwrap();
        let file = File::open(&path).unwrap();
        let id = removal_identity(&file).unwrap();
        let hash = content_hash_bytes(&data);
        let revision = format!("{id}:{hash}");
        BEFORE_REMOVE.with(|h| {
            *h.borrow_mut() = Some(Box::new(|path| {
                fs::rename(path, path.with_extension("saved")).unwrap();
                fs::write(path, "stranger").unwrap()
            }))
        });
        assert!(matches!(
            claim_unlink(&dir, &path, &revision, &hash, data.len() as u64, None),
            Err(SyncError::Conflict)
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), "stranger");
        let saved = path.with_extension("saved");
        claim_unlink(&dir, &saved, &revision, &hash, data.len() as u64, None).unwrap();
        assert!(!saved.exists() && path.exists());
    }
    #[test]
    fn removal_unix_shared_inode_and_unprotected_directory_are_unsupported() {
        let (root, p) = fixture();
        fs::hard_link(
            root.join(format!("StudyApp/notes/{ID}.md")),
            root.join("external-alias"),
        )
        .unwrap();
        assert!(matches!(
            p.inspect_note(ID),
            Err(SyncError::UnsupportedRemoval(_))
        ));
        let (root, p) = fixture();
        fs::set_permissions(
            root.join("StudyApp/notes"),
            fs::Permissions::from_mode(0o777),
        )
        .unwrap();
        assert!(matches!(
            p.inspect_note(ID),
            Err(SyncError::UnsupportedRemoval(_))
        ));
        assert!(p.read(ID).unwrap().is_some());
    }
}
