use crate::*;
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

/// File locks coordinate all StudyApp writers, including separate processes.
/// A NAS must implement OS file locks correctly. Replacements retain the old
/// object, recheck its moved bytes and publish create-only. Windows also denies
/// concurrent writers while moving the original. No original/staging is deleted.
pub struct LocalFolderProvider {
    root: PathBuf,
}

#[cfg(test)]
type RaceHook = Box<dyn FnOnce(&Path)>;
#[cfg(test)]
thread_local! {
    pub(crate) static BEFORE_RETAIN: std::cell::RefCell<Option<RaceHook>> = const { std::cell::RefCell::new(None) };
    pub(crate) static BEFORE_PUBLISH: std::cell::RefCell<Option<RaceHook>> = const { std::cell::RefCell::new(None) };
}

fn checked_metadata(path: &Path) -> Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(SyncError::InvalidPath);
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                // Includes junctions and other reparse points, not just symlinks.
                if metadata.file_attributes() & 0x400 != 0 {
                    return Err(SyncError::InvalidPath);
                }
            }
            Ok(Some(metadata))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

impl LocalFolderProvider {
    fn asset_path(&self, key: &str, create: bool) -> Result<PathBuf> {
        validate_asset_key(key)?;
        self.ensure_dirs()?;
        let mut path = self.root.join("StudyApp");
        let parts: Vec<_> = key.split('/').collect();
        for part in &parts[..2] {
            path.push(part);
            match checked_metadata(&path)? {
                Some(m) if m.is_dir() => {}
                Some(_) => return Err(SyncError::InvalidPath),
                None if create => {
                    fs::create_dir(&path)?;
                }
                None => return Ok(path.join(parts[2])),
            }
            if !checked_metadata(&path)?.is_some_and(|m| m.is_dir())
                || !fs::canonicalize(&path)?.starts_with(&self.root)
            {
                return Err(SyncError::InvalidPath);
            }
        }
        Ok(path.join(parts[2]))
    }
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        // The chosen parent must exist; never create directories outside it.
        let absolute = std::path::absolute(root)?;
        for ancestor in absolute.ancestors() {
            if let Some(metadata) = checked_metadata(ancestor)? {
                if !metadata.is_dir() {
                    return Err(SyncError::InvalidPath);
                }
            } else {
                return Err(SyncError::InvalidPath);
            }
        }
        let root = fs::canonicalize(root)?;
        let provider = Self { root };
        provider.ensure_dirs()?;
        Ok(provider)
    }

    fn ensure_dirs(&self) -> Result<PathBuf> {
        // Recheck every path component on each operation; reject reparse points.
        for ancestor in self.root.ancestors() {
            if !checked_metadata(ancestor)?.is_some_and(|m| m.is_dir()) {
                return Err(SyncError::InvalidPath);
            }
        }
        let mut path = self.root.clone();
        for name in ["StudyApp", "notes"] {
            path.push(name);
            match checked_metadata(&path)? {
                Some(m) if m.is_dir() => {}
                Some(_) => return Err(SyncError::InvalidPath),
                None => match fs::create_dir(&path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(e.into()),
                },
            }
            if !checked_metadata(&path)?.is_some_and(|m| m.is_dir())
                || !fs::canonicalize(&path)?.starts_with(&self.root)
            {
                return Err(SyncError::InvalidPath);
            }
        }
        Ok(path)
    }

    fn locked_dir(&self) -> Result<(PathBuf, File)> {
        let notes = self.ensure_dirs()?;
        let lock_path = self.root.join("StudyApp/.sync.lock");
        if let Some(m) = checked_metadata(&lock_path)?
            && !m.is_file()
        {
            return Err(SyncError::InvalidPath);
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        lock.try_lock().map_err(|_| SyncError::LockBusy)?;
        self.ensure_dirs()?;
        Ok((notes, lock))
    }

    fn read_at(notes: &Path, id: &str) -> Result<Option<RemoteNote>> {
        let path = notes.join(format!("{id}.md"));
        Self::read_file(&path)
    }

    fn read_file(path: &Path) -> Result<Option<RemoteNote>> {
        let Some(metadata) = checked_metadata(path)? else {
            return Ok(None);
        };
        if !metadata.is_file() {
            return Err(SyncError::InvalidPath);
        }
        let content = read_note(File::open(path)?)?;
        validate_content(&content)?;
        let hash = content_hash(&content);
        Ok(Some(RemoteNote {
            content,
            revision: Some(hash.clone()),
            hash,
        }))
    }

    fn retained_dir(&self, notes: &Path) -> Result<PathBuf> {
        let retained = notes.join(".retained");
        match checked_metadata(&retained)? {
            Some(m) if m.is_dir() => {}
            Some(_) => return Err(SyncError::InvalidPath),
            None => match fs::create_dir(&retained) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            },
        }
        if !checked_metadata(&retained)?.is_some_and(|m| m.is_dir())
            || !fs::canonicalize(&retained)?.starts_with(&self.root)
        {
            return Err(SyncError::InvalidPath);
        }
        Ok(retained)
    }
}

impl Provider for LocalFolderProvider {
    fn read_asset(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let (_notes, _lock) = self.locked_dir()?;
        let path = self.asset_path(key, false)?;
        let Some(metadata) = checked_metadata(&path)? else {
            return Ok(None);
        };
        if !metadata.is_file() {
            return Err(SyncError::InvalidPath);
        }
        if metadata.len() > MAX_CONTENT_BYTES as u64 {
            return Err(SyncError::LimitExceeded);
        }
        bounded_bytes(File::open(path)?).map(Some)
    }
    fn write_asset(&self, key: &str, bytes: &[u8]) -> Result<()> {
        if bytes.len() > MAX_CONTENT_BYTES {
            return Err(SyncError::LimitExceeded);
        }
        let (notes, _lock) = self.locked_dir()?;
        let target = self.asset_path(key, true)?;
        if checked_metadata(&target)?.is_some() {
            return Err(SyncError::Conflict);
        }
        let staged = self
            .retained_dir(&notes)?
            .join(format!("{}.asset-pending", uuid::Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::hard_link(staged, target).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                SyncError::Conflict
            } else {
                e.into()
            }
        })?;
        Ok(())
    }
    fn list(&self) -> Result<Vec<String>> {
        let (notes, _lock) = self.locked_dir()?;
        let mut ids = Vec::new();
        let mut scanned = 0usize;
        for entry in fs::read_dir(notes)? {
            scanned += 1;
            if scanned > MAX_ENTRIES {
                return Err(SyncError::LimitExceeded);
            }
            let entry = entry?;
            let name = entry.file_name();
            let Some(id) = name.to_str().and_then(|n| n.strip_suffix(".md")) else {
                continue;
            };
            if validate_id(id).is_err() {
                continue;
            }
            if checked_metadata(&entry.path())?.is_some_and(|m| m.is_file()) {
                ids.push(id.to_owned());
            }
        }
        ids.sort();
        Ok(ids)
    }
    fn read(&self, id: &str) -> Result<Option<RemoteNote>> {
        validate_id(id)?;
        let (notes, _lock) = self.locked_dir()?;
        Self::read_at(&notes, id)
    }
    fn write(
        &self,
        id: &str,
        content: &str,
        expected_revision: Option<&str>,
    ) -> Result<RemoteNote> {
        validate_id(id)?;
        validate_content(content)?;
        let (notes, _lock) = self.locked_dir()?;
        let current = Self::read_at(&notes, id)?;
        if current.as_ref().and_then(|n| n.revision.as_deref()) != expected_revision {
            return Err(SyncError::Conflict);
        }
        let target = notes.join(format!("{id}.md"));
        // Retain an incomplete staging file on failure for recovery; ignored by
        // listing. No cleanup operation ever removes a user directory.
        let retained = self.retained_dir(&notes)?;
        let staging = retained.join(format!("{}.pending", uuid::Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        drop(file);
        let current = Self::read_at(&notes, id)?;
        if current.as_ref().and_then(|n| n.revision.as_deref()) != expected_revision {
            return Err(SyncError::Conflict);
        }
        let original = if expected_revision.is_some() {
            let mut options = OpenOptions::new();
            options.read(true);
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                // Share read/delete for verification/rename, but deny writers.
                options.share_mode(5);
            }
            let original = options.open(&target)?;
            let actual = read_note(&original)?;
            if Some(content_hash(&actual).as_str()) != expected_revision {
                return Err(SyncError::Conflict);
            }
            let backup = retained.join(format!("{}.original", uuid::Uuid::new_v4()));
            #[cfg(test)]
            BEFORE_RETAIN.with(|hook| {
                if let Some(hook) = hook.borrow_mut().take() {
                    hook(&target);
                }
            });
            fs::rename(&target, &backup)?;
            // A different process can replace the path while the old handle is
            // open. Verify the object we actually moved, not just that handle.
            let moved = Self::read_file(&backup);
            let matches = moved
                .as_ref()
                .ok()
                .and_then(|n| n.as_ref())
                .and_then(|n| n.revision.as_deref())
                == expected_revision;
            if !matches {
                // Restore create-only if still vacant; never replace a newer
                // target. The original also remains in .retained in either case.
                match fs::hard_link(&backup, &target) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error.into()),
                }
                return Err(moved.err().unwrap_or(SyncError::Conflict));
            }
            Some(original)
        } else {
            None
        };
        // Atomic create-only publication also refuses targets created after the
        // old file was retained; staging and original survive every failure.
        #[cfg(test)]
        BEFORE_PUBLISH.with(|hook| {
            if let Some(hook) = hook.borrow_mut().take() {
                hook(&target);
            }
        });
        fs::hard_link(&staging, &target).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                SyncError::Conflict
            } else {
                e.into()
            }
        })?;
        OpenOptions::new().write(true).open(&target)?.sync_all()?;
        #[cfg(unix)]
        {
            File::open(&retained)?.sync_all()?;
            File::open(&notes)?.sync_all()?;
        }
        drop(original);
        let hash = content_hash(content);
        Ok(RemoteNote {
            content: content.to_owned(),
            revision: Some(hash.clone()),
            hash,
        })
    }
    fn test_connection(&self) -> Result<()> {
        let _ = self.locked_dir()?;
        self.list().map(|_| ())
    }
}
