//! Only fixed, header-verified single-link files may be written or rotated.
use anyhow::{Context, Result, ensure};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

pub(super) const SLOT_COUNT: usize = 5;
pub(super) const SLOT_BYTES: u64 = 2 * 1024 * 1024;
const MARKER: &str = "WW-StudyApp local logs v1\n";
const LOCK: &[u8] = b"WW-StudyApp log lock v1\n";
const MARKER_NAME: &str = ".studyapp-log-owner";
const LOCK_NAME: &str = ".studyapp-log-lock";

fn single_link(file: &File) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::{fs::MetadataExt, io::AsRawHandle};
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };
        ensure!(
            file.metadata()?.file_attributes() & 0x400 == 0,
            "日志文件不能为重解析点"
        );
        let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
        // SAFETY: file keeps the handle alive and info has the exact API output layout.
        ensure!(
            unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) } != 0,
            "无法核对日志文件链接"
        );
        // SAFETY: successful API call initialized info.
        ensure!(
            unsafe { info.assume_init() }.nNumberOfLinks == 1,
            "日志文件不能为硬链接"
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(file.metadata()?.nlink() == 1, "日志文件不能为硬链接");
    }
    ensure!(file.metadata()?.is_file(), "日志路径不是普通文件");
    Ok(())
}
fn read_only(path: &Path) -> Result<File> {
    let metadata = std::fs::symlink_metadata(path)?;
    ensure!(metadata.file_type().is_file(), "日志路径不是普通文件");
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000).share_mode(3);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    single_link(&file)?;
    Ok(file)
}
fn opened(path: &Path, create: bool) -> Result<File> {
    let before = if create { None } else { Some(read_only(path)?) };
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(create);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000).share_mode(3);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    single_link(&file)?;
    let current = read_only(path)?;
    let identity = crate::app_usage::identity(&file)?;
    ensure!(
        identity == crate::app_usage::identity(&current)?,
        "日志文件身份发生变化"
    );
    if let Some(before) = before {
        ensure!(
            identity == crate::app_usage::identity(&before)?,
            "日志文件身份发生变化"
        );
    }
    Ok(file)
}
fn exists(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}
fn header(file: &mut File, expected: &[u8]) -> Result<()> {
    single_link(file)?;
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = vec![0; expected.len()];
    file.read_exact(&mut bytes)?;
    ensure!(bytes == expected, "日志文件头不属于此日志目录");
    ensure!(
        file.metadata()?.len() <= SLOT_BYTES,
        "日志文件超过固定容量，原文件已保留"
    );
    Ok(())
}
fn owner(path: &Path) -> Result<uuid::Uuid> {
    let file = read_only(path)?;
    single_link(&file)?;
    ensure!(file.metadata()?.len() <= 128, "日志所有权记录无效");
    let mut bytes = String::new();
    file.take(129).read_to_string(&mut bytes)?;
    let id = bytes
        .strip_prefix(MARKER)
        .and_then(|s| s.strip_suffix('\n'))
        .context("日志目录未登记")?;
    let uuid = uuid::Uuid::parse_str(id)?;
    ensure!(id == uuid.to_string(), "日志所有权记录无效");
    Ok(uuid)
}
fn slot_name(index: usize) -> String {
    format!("studyapp-{index}.jsonl")
}
fn slot_header(id: uuid::Uuid, index: usize) -> Vec<u8> {
    let header =
        format!(r#"{{"format":"ww-studyapp-log","schema":1,"owner":"{id}","slot":{index}}}"#);
    let mut bytes = header.into_bytes();
    bytes.push(10);
    bytes
}

pub(super) struct Sink {
    directory: PathBuf,
    _guards: Vec<File>,
    _lock: File,
    _marker: File,
    owner: uuid::Uuid,
    index: usize,
    active: Option<File>,
    limit: u64,
}
impl Sink {
    pub(super) fn open(directory: &Path) -> Result<Self> {
        Self::with_limit(directory, SLOT_BYTES)
    }
    fn with_limit(directory: &Path, limit: u64) -> Result<Self> {
        let guards = crate::storage_backend::guard_directories(directory, true)?;
        let lock_path = directory.join(LOCK_NAME);
        let mut lock = if exists(&lock_path)? {
            let mut file = opened(&lock_path, false)?;
            header(&mut file, LOCK)?;
            ensure!(
                file.metadata()?.len() == LOCK.len() as u64,
                "日志锁记录无效"
            );
            file
        } else {
            let mut file = opened(&lock_path, true)?;
            file.write_all(LOCK)?;
            file.sync_all()?;
            file
        };
        lock.try_lock().context("另一实例正在写入此日志目录")?;
        // Revalidate after acquiring the lock; never remove or steal a lock file.
        header(&mut lock, LOCK)?;
        let marker_path = directory.join(MARKER_NAME);
        if !exists(&marker_path)? {
            // Unknown slots must not become ours merely by creating a marker.
            for index in 0..SLOT_COUNT {
                ensure!(
                    !exists(&directory.join(slot_name(index)))?,
                    "未登记的同名日志文件已保留"
                );
            }
            let mut marker = opened(&marker_path, true)?;
            let bytes = format!("{MARKER}{}\n", uuid::Uuid::new_v4());
            marker.write_all(bytes.as_bytes())?;
            marker.sync_all()?;
        }
        let id = owner(&marker_path)?;
        let marker = opened(&marker_path, false)?;
        let mut latest = None;
        for index in 0..SLOT_COUNT {
            let path = directory.join(slot_name(index));
            if exists(&path)? {
                let mut file = opened(&path, false)?;
                header(&mut file, &slot_header(id, index))?;
                ensure!(
                    file.metadata()?.len() <= limit,
                    "日志槽位超过容量，原文件已保留"
                );
                let time = file.metadata()?.modified()?;
                if latest.is_none_or(|(_, previous)| time > previous) {
                    latest = Some((index, time));
                }
            }
        }
        let mut sink = Self {
            directory: directory.into(),
            _guards: guards,
            _lock: lock,
            _marker: marker,
            owner: id,
            index: latest.map_or(0, |(index, _)| index),
            active: None,
            limit,
        };
        sink.active = Some(sink.open_slot(sink.index, false)?);
        Ok(sink)
    }
    fn open_slot(&self, index: usize, rotate: bool) -> Result<File> {
        let path = self.directory.join(slot_name(index));
        let expected = slot_header(self.owner, index);
        let mut file = if exists(&path)? {
            let mut file = opened(&path, false)?;
            header(&mut file, &expected)?;
            if rotate {
                file.set_len(0)?;
                file.seek(SeekFrom::Start(0))?;
                file.write_all(&expected)?;
            }
            file
        } else {
            let mut file = opened(&path, true)?;
            file.write_all(&expected)?;
            file
        };
        file.seek(SeekFrom::End(0))?;
        Ok(file)
    }
    pub(super) fn write(&mut self, bytes: &[u8]) -> Result<()> {
        ensure!(bytes.len() <= 4096, "日志记录超过固定容量");
        let mut file = self.active.take().context("日志句柄不可用")?;
        // A hard link added since open must not permit a later write/truncation.
        single_link(&file)?;
        if file.metadata()?.len() + bytes.len() as u64 > self.limit {
            file.sync_all()?;
            drop(file);
            self.index = (self.index + 1) % SLOT_COUNT;
            file = self.open_slot(self.index, true)?;
        }
        ensure!(
            file.metadata()?.len() + bytes.len() as u64 <= self.limit,
            "日志记录超过槽位容量"
        );
        file.write_all(bytes)?;
        self.active = Some(file);
        Ok(())
    }
    pub(super) fn flush(&mut self) -> Result<()> {
        if let Some(file) = &self.active {
            single_link(file)?;
            file.sync_all()?;
        }
        Ok(())
    }
}
/// Read only explicitly owned files, never recurse through a user-selected directory.
pub(crate) fn inventory(directory: &Path) -> Result<Vec<PathBuf>> {
    if !exists(directory)? {
        return Ok(Vec::new());
    }
    let _guards = crate::storage_backend::guard_directories(directory, false)?;
    let id = owner(&directory.join(MARKER_NAME))?;
    let lock = read_only(&directory.join(LOCK_NAME))?;
    // Windows locks cover file bytes, so inspect this lease by metadata only.
    single_link(&lock)?;
    ensure!(
        lock.metadata()?.len() == LOCK.len() as u64,
        "日志锁记录无效"
    );
    let mut result = vec![directory.join(MARKER_NAME), directory.join(LOCK_NAME)];
    for index in 0..SLOT_COUNT {
        let path = directory.join(slot_name(index));
        if exists(&path)? {
            let mut file = read_only(&path)?;
            header(&mut file, &slot_header(id, index))?;
            result.push(path);
        }
    }
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixed_ring_is_bounded_and_unknown_files_are_never_truncated() {
        let root =
            tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        let unrelated = root.path().join("notes.txt");
        std::fs::write(&unrelated, "private user data").unwrap();
        let mut sink = Sink::with_limit(root.path(), 300).unwrap();
        for _ in 0..100 {
            sink.write(b"{\"fixed\":true,\"count\":1}\n").unwrap();
        }
        sink.flush().unwrap();
        let files = inventory(root.path()).unwrap();
        assert_eq!(files.len(), SLOT_COUNT + 2);
        for index in 0..SLOT_COUNT {
            assert!(
                std::fs::metadata(root.path().join(slot_name(index)))
                    .unwrap()
                    .len()
                    <= 300
            );
        }
        assert_eq!(
            std::fs::read_to_string(unrelated).unwrap(),
            "private user data"
        );
        drop(sink);
        std::fs::write(root.path().join(slot_name(2)), "unknown same-name record").unwrap();
        assert!(Sink::open(root.path()).is_err());
        assert_eq!(
            std::fs::read_to_string(root.path().join(slot_name(2))).unwrap(),
            "unknown same-name record"
        );
    }
    #[test]
    fn busy_directory_and_unknown_lock_or_slot_fail_without_stealing() {
        let root =
            tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        let sink = Sink::open(root.path()).unwrap();
        assert!(Sink::open(root.path()).is_err());
        drop(sink);
        assert!(Sink::open(root.path()).is_ok());
        let other =
            tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        std::fs::write(other.path().join(slot_name(0)), "unknown").unwrap();
        assert!(Sink::open(other.path()).is_err());
        assert!(!other.path().join(MARKER_NAME).exists());
        assert_eq!(
            std::fs::read_to_string(other.path().join(slot_name(0))).unwrap(),
            "unknown"
        );
        let other =
            tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        std::fs::write(other.path().join(LOCK_NAME), "unknown lock").unwrap();
        assert!(Sink::open(other.path()).is_err());
        assert_eq!(
            std::fs::read_to_string(other.path().join(LOCK_NAME)).unwrap(),
            "unknown lock"
        );
    }
    #[test]
    fn hard_linked_slot_is_rejected_even_if_link_is_added_after_open() {
        let root =
            tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        let mut sink = Sink::open(root.path()).unwrap();
        let slot = root.path().join(slot_name(0));
        let before = std::fs::read(&slot).unwrap();
        std::fs::hard_link(&slot, root.path().join("alias")).unwrap();
        assert!(sink.write(b"{}\n").is_err());
        drop(sink);
        assert!(Sink::open(root.path()).is_err());
        assert!(inventory(root.path()).is_err());
        assert_eq!(std::fs::read(slot).unwrap(), before);
    }
    #[cfg(windows)]
    #[test]
    fn windows_junction_parent_is_rejected_without_following_or_creating_child() {
        use std::os::windows::process::CommandExt;
        let root = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let link = root.path().join("junction");
        let output = std::process::Command::new("cmd.exe")
            .args(["/c", "mklink", "/J"])
            .arg(&link)
            .arg(target.path())
            .creation_flags(0x0800_0000)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(Sink::open(&link.join("logs")).is_err());
        assert!(!target.path().join("logs").exists());
    }
    #[cfg(unix)]
    #[test]
    fn symlinks_in_parents_or_slots_are_not_followed() {
        use std::os::unix::fs::symlink;
        let root =
            tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        let target =
            tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        symlink(target.path(), root.path().join("linked")).unwrap();
        assert!(Sink::open(&root.path().join("linked/logs")).is_err());
        assert!(!target.path().join("logs").exists());
        std::fs::write(target.path().join("private"), "secret").unwrap();
        symlink(
            target.path().join("private"),
            root.path().join(slot_name(0)),
        )
        .unwrap();
        assert!(Sink::open(root.path()).is_err());
        assert_eq!(
            std::fs::read_to_string(target.path().join("private")).unwrap(),
            "secret"
        );
    }
}
