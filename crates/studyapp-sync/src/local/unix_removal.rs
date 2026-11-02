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
// Ordinary reads check the single recovery slot in constant time. Missing
// entries also check for legacy claims that predate durable receipts.
pub(super) fn recover_provider_dir(
    provider: &LocalFolderProvider,
    folder: &Path,
    legacy: bool,
) -> Result<()> {
    if !folder.try_exists()? {
        return Ok(());
    }
    let needs_slot = folder.join(SLOT).try_exists()?;
    if !needs_slot
        && (!legacy
            || !fs::read_dir(folder)?.any(|e| {
                e.map_or(true, |e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with(".studyapp-removal-")
                })
            }))
    {
        return Ok(());
    }
    let dir = parent(provider, &folder.join("sentinel"))?;
    if legacy {
        recover_removal_dir(&dir, folder)
    } else {
        recover_slot(&dir, folder)
    }
}
fn inspect_locked(
    provider: &LocalFolderProvider,
    target: &str,
    asset: bool,
) -> Result<RemoteRemoval> {
    let path = provider.removal_path(target, asset)?;
    recover_provider_dir(provider, path.parent().ok_or(SyncError::InvalidPath)?, true)?;
    recover_provider_dir(
        provider,
        &provider.root.join("StudyApp/notes/.retained"),
        true,
    )?;
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
    dir.sync_all()?;
    syscall(unsafe { unlinkat(txn.as_raw_fd(), claimed.as_ptr(), 0) })?;
    txn.sync_all()?;
    Ok(())
}
const SLOT: &str = ".studyapp-removal-v1";
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaimReceipt {
    name: Vec<u8>,
    parent: String,
    transaction: String,
    revision: String,
    hash: String,
    bytes: u64,
}
fn missing(e: &SyncError) -> bool {
    matches!(e, SyncError::Io(std::io::ErrorKind::NotFound))
}
fn recovery_error(folder: &Path) -> SyncError {
    SyncError::RemovalRecovery(
        folder
            .join(SLOT)
            .join("claimed")
            .to_string_lossy()
            .into_owned(),
    )
}
fn write_receipt(dir: &File, txn: &File, receipt: &ClaimReceipt) -> Result<()> {
    let temporary = CString::new(format!("receipt-{}", uuid::Uuid::new_v4())).unwrap();
    #[cfg(target_os = "linux")]
    let create = 0x40 | 0x80;
    #[cfg(target_os = "macos")]
    let create = 0x200 | 0x800;
    let fd = unsafe {
        openat(
            txn.as_raw_fd(),
            temporary.as_ptr(),
            1 | create | NOFOLLOW | CLOEXEC,
            0o600 as c_int,
        )
    };
    syscall(fd)?;
    let mut file = unsafe { File::from_raw_fd(fd) };
    serde_json::to_writer(&mut file, receipt).map_err(|_| SyncError::InvalidContent)?;
    file.sync_all()?;
    syscall(unsafe {
        renameat(
            txn.as_raw_fd(),
            temporary.as_ptr(),
            txn.as_raw_fd(),
            c"receipt.json".as_ptr(),
        )
    })?;
    txn.sync_all()?;
    dir.sync_all()?;
    Ok(())
}
// Return the claimed bytes to their original name before a fresh ownership /
// reference preview. Never discard a stranger or replace a newly occupied name.
fn recover_locked(dir: &File, txn: &File, folder: &Path) -> Result<()> {
    let claimed = match open_at(txn, std::ffi::OsStr::new("claimed"), false) {
        Ok(file) => file,
        Err(e) if missing(&e) => return Ok(()),
        Err(_) => return Err(recovery_error(folder)),
    };
    let evidence = || -> Result<_> {
        let f = open_at(txn, std::ffi::OsStr::new("receipt.json"), false)?;
        if f.metadata()?.len() > 16384 {
            return Err(SyncError::InvalidContent);
        }
        let receipt: ClaimReceipt =
            serde_json::from_reader(f.take(16385)).map_err(|_| SyncError::InvalidContent)?;
        let part = std::ffi::OsStr::from_bytes(&receipt.name);
        if Path::new(part).file_name() != Some(part)
            || receipt.parent != removal_identity(dir)?
            || receipt.transaction != removal_identity(txn)?
        {
            return Err(SyncError::InvalidPath);
        }
        let id = removal_identity(&claimed)?;
        if !claimed.metadata()?.is_file() || claimed.metadata()?.len() != receipt.bytes {
            return Err(SyncError::Conflict);
        }
        let (hash, len) = removal_hash(&claimed, receipt.bytes)?;
        if receipt.revision != format!("{id}:{hash}")
            || receipt.hash != hash
            || receipt.bytes != len
        {
            return Err(SyncError::Conflict);
        }
        Ok((receipt, id))
    };
    let (receipt, id) = evidence().map_err(|_| recovery_error(folder))?;
    let part = std::ffi::OsStr::from_bytes(&receipt.name);
    let original = name(part)?;
    let existing = open_at(dir, part, false);
    match existing {
        Ok(file) if removal_identity(&file)? == id => {}
        Err(e) if missing(&e) => {
            if unsafe {
                linkat(
                    txn.as_raw_fd(),
                    c"claimed".as_ptr(),
                    dir.as_raw_fd(),
                    original.as_ptr(),
                    0,
                )
            } < 0
            {
                return Err(recovery_error(folder));
            }
        }
        _ => return Err(recovery_error(folder)),
    }
    dir.sync_all()?;
    syscall(unsafe { unlinkat(txn.as_raw_fd(), c"claimed".as_ptr(), 0) })?;
    txn.sync_all()?;
    Ok(())
}
fn recover_slot(dir: &File, folder: &Path) -> Result<()> {
    let txn = match open_at(dir, std::ffi::OsStr::new(SLOT), true) {
        Ok(txn) => txn,
        Err(e) if missing(&e) => return Ok(()),
        Err(e) => return Err(e),
    };
    owned(dir)?;
    owned(&txn)?;
    if txn.metadata()?.mode() & 0o777 != 0o700 {
        return Err(recovery_error(folder));
    }
    txn.try_lock().map_err(|_| SyncError::LockBusy)?;
    recover_locked(dir, &txn, folder)
}
/// Recover previously confirmed, interrupted claims, without deleting their
/// contents. Legacy private claims without a durable receipt need manual recovery.
pub fn recover_removal_dir(dir: &File, folder: &Path) -> Result<()> {
    recover_slot(dir, folder)?;
    if removal_identity(dir)? != removal_identity(&File::open(folder)?)? {
        return Err(SyncError::InvalidPath);
    }
    for (n, e) in fs::read_dir(folder)?.enumerate() {
        if n >= 100_000 {
            return Err(SyncError::LimitExceeded);
        }
        let part = e?.file_name();
        let Some(s) = part.to_str() else { continue };
        if !s
            .strip_prefix(".studyapp-removal-")
            .is_some_and(|tail| uuid::Uuid::parse_str(tail).is_ok())
        {
            continue;
        }
        let old = open_at(dir, &part, true)?;
        match open_at(&old, std::ffi::OsStr::new("claimed"), false) {
            Err(e) if missing(&e) => {}
            _ => {
                return Err(SyncError::RemovalRecovery(
                    folder
                        .join(part)
                        .join("claimed")
                        .to_string_lossy()
                        .into_owned(),
                ));
            }
        }
    }
    Ok(())
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
    let folder = path.parent().ok_or(SyncError::InvalidPath)?;
    let txn_name = name(std::ffi::OsStr::new(SLOT))?;
    if unsafe { mkdirat(dir.as_raw_fd(), txn_name.as_ptr(), 0o700) } < 0
        && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
    {
        return Err(std::io::Error::last_os_error().into());
    }
    let txn = open_at(dir, std::ffi::OsStr::new(SLOT), true)?;
    owned(&txn)?;
    if txn.metadata()?.mode() & 0o777 != 0o700 {
        return Err(SyncError::UnsupportedRemoval(
            "Unix transaction directory must have exact 0700 permissions",
        ));
    }
    txn.try_lock().map_err(|_| SyncError::LockBusy)?;
    recover_locked(dir, &txn, folder)?;
    write_receipt(
        dir,
        &txn,
        &ClaimReceipt {
            name: original.as_bytes().to_vec(),
            parent: removal_identity(dir)?,
            transaction: removal_identity(&txn)?,
            revision: revision.into(),
            hash: hash.into(),
            bytes,
        },
    )?;
    let recovery = folder.join(SLOT).join("claimed");
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
    txn.sync_all()?;
    dir.sync_all()?;
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
    txn.sync_all()?;
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
    fn removal_unix_repreview_after_interrupted_claim_preserves_replacement_and_completes() {
        let (root, p) = fixture();
        let notes = root.join("StudyApp/notes");
        let target = notes.join(format!("{ID}.md"));
        let mut plan = p.inspect_note(ID).unwrap();
        plan.confirmed_retained.clear();
        AFTER_CLAIM.with(|h| {
            *h.borrow_mut() = Some(Box::new(|_| {
                panic!("simulate process stopping after durable rename")
            }))
        });
        let interrupted = std::panic::catch_unwind(|| p.delete_note(&plan));
        assert!(interrupted.is_err() && !target.exists());
        let private = notes.join(SLOT).join("claimed");
        assert_eq!(fs::read_to_string(&private).unwrap(), "note");
        drop(p);
        let p = LocalFolderProvider::new(&root).unwrap();
        fs::write(&target, "new owner").unwrap();
        let Err(SyncError::RemovalRecovery(path)) = p.read(ID) else {
            panic!("collision must not report absence")
        };
        assert_eq!(Path::new(&path), private);
        assert_eq!(fs::read_to_string(&target).unwrap(), "new owner");
        fs::rename(&target, root.join("preserved stranger")).unwrap();
        assert_eq!(p.read(ID).unwrap().unwrap().content, "note");
        assert!(!private.exists());
        let resumed = p.inspect_note(ID).unwrap();
        p.delete_note(&resumed).unwrap();
        assert!(p.read(ID).unwrap().is_none());
        assert!(!private.exists());
        assert_eq!(
            fs::read_to_string(root.join("preserved stranger")).unwrap(),
            "new owner"
        );
    }
    #[test]
    fn missing_source_with_unjournaled_legacy_claim_is_not_absent() {
        let (root, p) = fixture();
        let notes = root.join("StudyApp/notes");
        let tx = notes.join(format!(".studyapp-removal-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&tx).unwrap();
        let claimed = tx.join("claimed");
        fs::rename(notes.join(format!("{ID}.md")), &claimed).unwrap();
        assert!(
            matches!(p.read(ID),Err(SyncError::RemovalRecovery(ref path)) if Path::new(path)==claimed)
        );
        assert!(matches!(p.list(), Err(SyncError::RemovalRecovery(_))));
        assert_eq!(fs::read_to_string(claimed).unwrap(), "note");
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
