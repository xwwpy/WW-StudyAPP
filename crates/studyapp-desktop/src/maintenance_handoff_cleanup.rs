//! Ownership is captured at creation; authenticated snapshots never adopt neighbors.
use super::*;
use std::io::{Seek, SeekFrom};
use studyapp_desktop::maintenance::normalize_relative_path;
const PREFIX: &str = "WW-StudyApp-maintenance-";
const JOURNAL: &str = "cleanup.json";
const CONTEXT: &str = "WW-StudyApp/Maintenance/cleanup/v2\n";
const MAX_STAGES: usize = 16;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CleanupFile {
    name: String,
    identity: FileIdentity,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CleanupRecord {
    schema: u32,
    root_identity: FileIdentity,
    files: Vec<CleanupFile>,
    owner_pid: u32,
    owner_created: u64,
    phase: StagePhase,
}
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
enum StagePhase {
    Prepared,
    Recoverable,
    Terminal,
}
fn stage_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|v| v.to_str())
        .and_then(|n| {
            n.strip_prefix(PREFIX)
                .or_else(|| n.strip_prefix("WW-StudyApp-Recovery-"))
        })
        .is_some_and(valid_reference)
}
fn allowed_name(name: &str) -> bool {
    matches!(
        name,
        "package.zip"
            | "package.msi"
            | "WW-StudyApp.exe"
            | "result.json"
            | "maintenance-result.png"
            | "Uninstall.exe"
            | "plan.json"
            | "recovery.bin"
            | "completed.bin"
            | "recovered.json"
            | "recovery-partial.json"
            | "status.json"
            | "journal.jsonl"
    ) || (name.ends_with(".dll") && !name.contains('/') && normalize_relative_path(name).is_ok())
        || name
            .strip_prefix("new-")
            .is_some_and(|s| s == "receipt" || s.parse::<usize>().is_ok())
        || name.strip_prefix("pending-").is_some_and(valid_reference)
}
fn validate_record(record: &CleanupRecord) -> Result<()> {
    ensure!(
        record.schema == 2
            && record.root_identity.file_id != 0
            && record.root_identity.size == 0
            && record.root_identity.sha256.is_empty()
            && record.files.len() <= 64
            && record.owner_pid > 0
            && record.owner_created > 0,
        "维护清理记录无效"
    );
    let mut names = std::collections::BTreeSet::new();
    for file in &record.files {
        ensure!(
            allowed_name(&file.name)
                && names.insert(file.name.to_lowercase())
                && valid_sha256(&file.identity.sha256)
                && file.identity.file_id != 0
                && file.identity.size <= 1024 * 1024 * 1024,
            "维护清理资源无效"
        );
    }
    Ok(())
}
fn encode(record: &CleanupRecord) -> Result<Vec<u8>> {
    validate_record(record)?;
    // DPAPI authenticates the bytes for this Windows account; the explicit
    // context prevents a credential blob from becoming maintenance authority.
    crate::credentials::protect(&format!("{CONTEXT}{}", serde_json::to_string(record)?))
        .context("无法保护维护所有权记录")
}
fn append_frame(file: &mut File, record: &CleanupRecord) -> Result<()> {
    let bytes = encode(record)?;
    ensure!(
        bytes.len() <= 65536 && file.metadata()?.len() < 1024 * 1024,
        "维护记录超过容量限制"
    );
    file.seek(SeekFrom::End(0))?;
    file.write_all(&(bytes.len() as u32).to_le_bytes())?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}
fn writable_journal(path: &Path, create: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(create);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1).custom_flags(0x0020_0000);
    }
    Ok(options.open(path)?)
}
pub(super) fn initialize_stage(stage: &Path) -> Result<()> {
    ensure!(stage_name(stage), "维护目录名称无效");
    let guards = crate::storage_backend::guard_directories(stage, false)?;
    let root_identity = crate::maintenance_runtime::handle_identity(
        guards.last().context("缺少维护目录身份")?,
        true,
    )?
    .0;
    let mut file = writable_journal(&stage.join(JOURNAL), true)?;
    let result = append_frame(
        &mut file,
        &CleanupRecord {
            schema: 2,
            root_identity: root_identity.clone(),
            files: vec![],
            owner_pid: std::process::id(),
            owner_created: native::current_process_created()?,
            phase: StagePhase::Prepared,
        },
    );
    if result.is_err() {
        // These bytes still belong to our create-new handle, even if encrypting
        // or flushing the first record failed. Never recapture a pathname.
        let identity = file
            .rewind()
            .ok()
            .and_then(|()| crate::maintenance_runtime::handle_identity(&file, false).ok())
            .map(|i| i.0);
        drop(file);
        if let Some(identity) = identity {
            let _ = crate::maintenance_execute::remove_exact_maintenance_file(
                &stage.join(JOURNAL),
                &identity,
            );
        }
        drop(guards);
        let _ = remove_empty_stage(stage, &root_identity);
    }
    result
}
fn read(stage: &Path) -> Result<(CleanupRecord, FileIdentity)> {
    let mut file = crate::storage_backend::plain_handle(&stage.join(JOURNAL), false, true)?;
    ensure!(file.metadata()?.len() <= 1024 * 1024, "维护清理记录过大");
    let (identity, links) = crate::maintenance_runtime::handle_identity(&file, false)?;
    ensure!(links == 1, "维护清理记录身份无效");
    file.rewind()?;
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
    let (mut offset, mut frames, mut latest) = (0, 0, None);
    while offset + 4 <= bytes.len() {
        let length = u32::from_le_bytes(bytes[offset..offset + 4].try_into()?) as usize;
        offset += 4;
        ensure!(length > 0 && length <= 65536, "维护记录帧无效");
        if offset + length > bytes.len() {
            break;
        }
        frames += 1;
        ensure!(frames <= 256, "维护记录帧过多");
        let plaintext = crate::credentials::unprotect(&bytes[offset..offset + length])
            .context("维护记录完整性验证失败")?;
        let body = plaintext
            .strip_prefix(CONTEXT)
            .context("维护记录上下文无效")?;
        let record: CleanupRecord = serde_json::from_str(body)?;
        validate_record(&record)?;
        latest = Some(record);
        offset += length;
    }
    Ok((latest.context("维护记录尚未完成持久化")?, identity))
}
fn checked_record(stage: &Path) -> Result<(Vec<File>, CleanupRecord, FileIdentity)> {
    ensure!(stage_name(stage), "维护目录名称无效");
    let guards = crate::storage_backend::guard_directories(stage, false)?;
    let (record, journal) = read(stage)?;
    ensure!(
        crate::maintenance_runtime::handle_identity(
            guards.last().context("缺少维护目录身份")?,
            true
        )?
        .0 == record.root_identity,
        "维护清理目录已改变"
    );
    Ok((guards, record, journal))
}
fn update_file(
    stage: &Path,
    name: &str,
    created: Option<&FileIdentity>,
    identity: FileIdentity,
) -> Result<()> {
    let (_guards, mut record, journal_identity) = checked_record(stage)?;
    if let Some(existing) = record.files.iter_mut().find(|f| f.name == name) {
        let created = created.context("维护资源已经登记")?;
        ensure!(
            existing.identity.volume == created.volume
                && existing.identity.file_id == created.file_id
                && identity.volume == created.volume
                && identity.file_id == created.file_id,
            "维护资源创建身份不匹配"
        );
        existing.identity = identity;
    } else {
        ensure!(created.is_none(), "维护资源创建记录已丢失");
        record.files.push(CleanupFile {
            name: name.into(),
            identity,
        });
    }
    validate_record(&record)?;
    let mut journal = writable_journal(&stage.join(JOURNAL), false)?;
    ensure!(
        crate::maintenance_runtime::handle_identity(&journal, false)?.0 == journal_identity,
        "维护清理记录在更新前已改变"
    );
    append_frame(&mut journal, &record)
}
fn perform(stage: &Path) -> Result<()> {
    let (guards, record, journal_identity) = checked_record(stage)?;
    ensure!(
        record.phase == StagePhase::Terminal
            || (record.phase == StagePhase::Prepared
                && !native::process_is_live(record.owner_pid, record.owner_created)?),
        "维护资源仍在使用或需要恢复"
    );
    for file in &record.files {
        let path = stage.join(&file.name);
        if path.try_exists()? {
            crate::maintenance_execute::remove_exact_maintenance_file(&path, &file.identity)?;
        }
    }
    crate::maintenance_execute::remove_exact_maintenance_file(
        &stage.join(JOURNAL),
        &journal_identity,
    )?;
    drop(guards);
    match remove_empty_stage(stage, &record.root_identity) {
        Ok(()) => Ok(()),
        Err(error) => {
            // Another live directory lease or an unknown neighbor can prevent
            // empty-directory removal. Preserve authenticated retry metadata.
            if let Ok(guards) = crate::storage_backend::guard_directories(stage, false)
                && guards.last().is_some_and(|f| {
                    crate::maintenance_runtime::handle_identity(f, true)
                        .is_ok_and(|i| i.0 == record.root_identity)
                })
                && let Ok(mut journal) = writable_journal(&stage.join(JOURNAL), true)
            {
                let _ = append_frame(&mut journal, &record);
            }
            Err(error)
        }
    }
}
/// The error guard must be declared before any writer; Rust then seals and
/// closes writers before this guard attempts exact-file cleanup.
pub(crate) struct StageErrorGuard {
    stage: PathBuf,
    armed: bool,
}
impl StageErrorGuard {
    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }
}
impl Drop for StageErrorGuard {
    fn drop(&mut self) {
        if self.armed
            && let Err(error) = abandon_current(&self.stage)
        {
            eprintln!(
                "WW-StudyApp: 临时维护资源已保留（{}）：{error:#}",
                self.stage.display()
            );
        }
    }
}
pub(crate) fn cleanup_stage_on_error(stage: &Path) -> Result<StageErrorGuard> {
    if !stage.join(JOURNAL).try_exists()? {
        initialize_stage(stage)?;
    }
    let _ = checked_record(stage)?;
    Ok(StageErrorGuard {
        stage: stage.into(),
        armed: true,
    })
}
pub(crate) struct TrackedStageFile {
    file: File,
    stage: PathBuf,
    name: String,
    created: FileIdentity,
    sealed: bool,
    _guards: Vec<File>,
}
impl std::ops::Deref for TrackedStageFile {
    type Target = File;
    fn deref(&self) -> &File {
        &self.file
    }
}
impl Read for TrackedStageFile {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.file.read(bytes)
    }
}
impl Write for TrackedStageFile {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.sealed = false;
        self.file.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}
impl Seek for TrackedStageFile {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.file.seek(position)
    }
}
impl TrackedStageFile {
    pub(crate) fn file_mut(&mut self) -> &mut File {
        self.sealed = false;
        &mut self.file
    }
    pub(crate) fn seal(&mut self) -> Result<FileIdentity> {
        self.file.sync_all()?;
        self.file.rewind()?;
        let (identity, links) = crate::maintenance_runtime::handle_identity(&self.file, false)?;
        ensure!(
            links == 1
                && identity.volume == self.created.volume
                && identity.file_id == self.created.file_id,
            "临时资源创建身份已改变"
        );
        if !self.sealed {
            update_file(
                &self.stage,
                &self.name,
                Some(&self.created),
                identity.clone(),
            )?;
            self.sealed = true;
        }
        Ok(identity)
    }
}
impl Drop for TrackedStageFile {
    fn drop(&mut self) {
        if let Err(error) = self.seal() {
            eprintln!("WW-StudyApp: 临时资源所有权封存未完成：{error:#}");
        }
    }
}
pub(crate) fn create_stage_file(stage: &Path, name: &str) -> Result<TrackedStageFile> {
    ensure!(allowed_name(name), "维护资源名称无效");
    let (guards, _, _) = checked_record(stage)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1).custom_flags(0x0020_0000);
    }
    let file = options.open(stage.join(name))?;
    let (created, links) = crate::maintenance_runtime::handle_identity(&file, false)?;
    ensure!(links == 1, "临时资源有多个链接");
    if let Err(error) = update_file(stage, name, None, created.clone()) {
        drop(file);
        let _ =
            crate::maintenance_execute::remove_exact_maintenance_file(&stage.join(name), &created);
        return Err(error);
    }
    Ok(TrackedStageFile {
        file,
        stage: stage.into(),
        name: name.into(),
        created,
        sealed: false,
        _guards: guards,
    })
}
pub(crate) fn cleanup_download_stage(
    stage: &Path,
    payload: &Path,
    expected: &FileIdentity,
) -> Result<()> {
    ensure!(
        same_absolute_path(payload.parent().context("下载文件目录无效")?, stage)?,
        "下载文件不属于维护目录"
    );
    let name = payload
        .file_name()
        .and_then(|p| p.to_str())
        .context("下载文件名称无效")?;
    ensure!(
        matches!(name, "package.zip" | "package.msi"),
        "下载文件名称无效"
    );
    let (_, record, _) = checked_record(stage)?;
    ensure!(
        record
            .files
            .iter()
            .any(|f| f.name == name && f.identity == *expected),
        "下载文件未记录创建身份"
    );
    transition(stage, None, StagePhase::Terminal)?;
    perform(stage)
}
/// This is validation only: never adopt a path just because a caller enumerated it.
pub(super) fn defer_helper_stage(stage: &Path, files: &[PathBuf]) -> Result<()> {
    let (_, record, _) = checked_record(stage)?;
    for path in files {
        ensure!(
            same_absolute_path(path.parent().context("维护资源目录无效")?, stage)?,
            "维护资源不属于暂存目录"
        );
        let name = path
            .file_name()
            .and_then(|p| p.to_str())
            .context("维护资源名称无效")?;
        let recorded = record
            .files
            .iter()
            .find(|f| f.name == name)
            .context("维护资源未记录创建身份")?;
        let observed = crate::maintenance_runtime::observe_file(path, name)?;
        ensure!(
            observed.link_count == 1 && observed.identity == recorded.identity,
            "维护资源身份已改变"
        );
    }
    transition(stage, None, StagePhase::Terminal)
}
fn transition(stage: &Path, expected_owner: Option<(u32, u64)>, phase: StagePhase) -> Result<()> {
    let (_guards, mut record, identity) = checked_record(stage)?;
    let expected =
        expected_owner.unwrap_or((std::process::id(), native::current_process_created()?));
    ensure!(
        (record.owner_pid, record.owner_created) == expected,
        "维护租约已移交"
    );
    record.owner_pid = std::process::id();
    record.owner_created = native::current_process_created()?;
    record.phase = phase;
    let mut file = writable_journal(&stage.join(JOURNAL), false)?;
    ensure!(
        crate::maintenance_runtime::handle_identity(&file, false)?.0 == identity,
        "维护租约记录已改变"
    );
    append_frame(&mut file, &record)
}
pub(crate) fn claim_stage(stage: &Path, pid: u32, created: u64) -> Result<()> {
    transition(stage, Some((pid, created)), StagePhase::Prepared)
}
pub(crate) fn mark_recoverable(stage: &Path) -> Result<()> {
    transition(stage, None, StagePhase::Recoverable)
}
pub(crate) fn claim_recovery_stage(stage: &Path) -> Result<()> {
    let (_, record, _) = checked_record(stage)?;
    ensure!(
        (record.owner_pid == std::process::id()
            && record.owner_created == native::current_process_created()?)
            || !native::process_is_live(record.owner_pid, record.owner_created)?,
        "原维护进程仍在运行"
    );
    transition(
        stage,
        Some((record.owner_pid, record.owner_created)),
        StagePhase::Recoverable,
    )
}
pub(crate) fn abandon_current(stage: &Path) -> Result<()> {
    let (_, record, _) = checked_record(stage)?;
    if record.phase != StagePhase::Prepared
        || record.owner_pid != std::process::id()
        || record.owner_created != native::current_process_created()?
    {
        return Ok(());
    }
    transition(stage, None, StagePhase::Terminal)?;
    perform(stage)
}
pub(crate) fn initialize_local_stage(stage: &Path) -> Result<()> {
    initialize_stage(stage)
}
pub(crate) fn retry_local_stages(root: &Path) -> Result<usize> {
    let mut retained = 0;
    for entry in std::fs::read_dir(root)?.take(1024) {
        let path = entry?.path();
        if !path
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.starts_with("WW-StudyApp-Recovery-"))
        {
            continue;
        }
        if checked_record(&path).is_err() {
            continue;
        }
        let _ = perform(&path);
        if path.exists() && still_owned(&path) {
            retained += 1;
        }
    }
    Ok(retained)
}
pub(crate) fn record_publication(stage: &Path, name: &str, identity: FileIdentity) -> Result<()> {
    ensure!(
        matches!(
            name,
            "completed.bin" | "recovered.json" | "recovery-partial.json"
        ),
        "维护发布名称无效"
    );
    let (_guards, mut record, journal_identity) = checked_record(stage)?;
    // A prior attempt may have registered the destination and died before its
    // non-replacing rename. Only an absent destination can change ownership;
    // retain the old pending object under its own captured record.
    ensure!(!stage.join(name).try_exists()?, "维护发布目标已存在");
    let pending = record
        .files
        .iter()
        .find(|file| file.name.starts_with("pending-") && file.identity == identity)
        .context("完成记录没有已封存的临时资源")?;
    let observed = crate::maintenance_runtime::observe_file(&stage.join(&pending.name), "pending")?;
    ensure!(
        observed.link_count == 1 && observed.identity == identity,
        "完成记录临时资源已改变"
    );
    if let Some(existing) = record.files.iter_mut().find(|file| file.name == name) {
        existing.identity = identity;
    } else {
        record.files.push(CleanupFile {
            name: name.into(),
            identity,
        });
    }
    validate_record(&record)?;
    let mut journal = writable_journal(&stage.join(JOURNAL), false)?;
    ensure!(
        crate::maintenance_runtime::handle_identity(&journal, false)?.0 == journal_identity,
        "维护发布记录已改变"
    );
    append_frame(&mut journal, &record)
}
pub(crate) fn recorded_files(stage: &Path) -> Result<Vec<(String, FileIdentity)>> {
    let (_, record, _) = checked_record(stage)?;
    Ok(record
        .files
        .into_iter()
        .map(|f| (f.name, f.identity))
        .collect())
}
fn still_owned(stage: &Path) -> bool {
    let Ok((_guards, record, _)) = checked_record(stage) else {
        return false;
    };
    let Ok(entries) = std::fs::read_dir(stage) else {
        return false;
    };
    for (count, entry) in entries.enumerate() {
        if count > 17 {
            return false;
        }
        let Ok(entry) = entry else {
            return false;
        };
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return false;
        };
        if name != JOURNAL
            && name != "journal.jsonl"
            && !record.files.iter().any(|f| f.name == name)
        {
            return false;
        }
    }
    for file in record.files {
        let path = stage.join(&file.name);
        match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return false,
            Ok(_) => {}
        }
        match crate::maintenance_runtime::observe_file(&path, &file.name) {
            Ok(actual) if actual.link_count == 1 && actual.identity == file.identity => {}
            Ok(_) => return false,
            // A directory, reparse point, unreadable replacement or active
            // writer is not proof of the captured immutable file. Preserve
            // it, but do not let unverifiable neighbors consume the quota.
            Err(_) => return false,
        }
    }
    true
}
pub(super) fn retry_before_create(root: &Path) -> Result<()> {
    let _guards = crate::storage_backend::guard_directories(root, false)?;
    let mut retained = 0;
    // Unknown/tampered directories are not adopted and cannot exhaust the quota
    // for authenticated, currently occupied maintenance work.
    for entry in std::fs::read_dir(root)? {
        let stage = entry?.path();
        if stage_name(&stage) && perform(&stage).is_err() && still_owned(&stage) {
            retained += 1;
        }
        ensure!(
            retained < MAX_STAGES,
            "暂存清理尚未完成；请关闭占用程序并重试"
        );
    }
    Ok(())
}
#[cfg(windows)]
fn remove_empty_stage(stage: &Path, expected: &FileIdentity) -> Result<()> {
    use std::os::windows::{
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle,
    };
    let _guards = crate::storage_backend::guard_directories(
        stage.parent().context("暂存父目录无效")?,
        false,
    )?;
    let file = OpenOptions::new()
        .read(true)
        .access_mode(0x8000_0000 | 0x0001_0000)
        .share_mode(3)
        .custom_flags(0x0020_0000 | 0x0200_0000)
        .open(stage)?;
    ensure!(
        file.metadata()?.file_attributes() & 0x400 == 0
            && crate::maintenance_runtime::handle_identity(&file, true)?.0 == *expected,
        "暂存目录身份已改变"
    );
    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: DELETE-capable directory handle, exact native input layout; Windows
    // rejects nonempty directories, so unknown neighbors can never be removed.
    ensure!(
        unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle(),
                FileDispositionInfo,
                (&info as *const FILE_DISPOSITION_INFO).cast(),
                std::mem::size_of_val(&info) as u32,
            )
        } != 0,
        "暂存目录仍有保留资源"
    );
    Ok(())
}
#[cfg(not(windows))]
fn remove_empty_stage(_stage: &Path, _expected: &FileIdentity) -> Result<()> {
    anyhow::bail!("维护清理仅支持 Windows")
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    fn fixture() -> PathBuf {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .join(".scratch/installation-20261026/test-temp");
        std::fs::create_dir_all(&parent).unwrap();
        let stage = parent.join(format!("{PREFIX}{}", uuid::Uuid::new_v4().simple()));
        create_private_directory(&stage).unwrap();
        initialize_stage(&stage).unwrap();
        stage
    }
    #[test]
    fn maintenance_handoff_dead_owner_reclaimed_but_live_or_recoverable_lease_retained() {
        let stage = fixture();
        let root = stage.parent().unwrap();
        let mut writer = create_stage_file(&stage, "package.msi").unwrap();
        writer.write_all(b"abandoned fixture").unwrap();
        writer.seal().unwrap();
        drop(writer);
        assert!(perform(&stage).is_err());
        assert!(stage.join("package.msi").exists());
        let (_, mut record, _) = checked_record(&stage).unwrap();
        record.owner_created -= 1;
        let mut journal = writable_journal(&stage.join(JOURNAL), false).unwrap();
        append_frame(&mut journal, &record).unwrap();
        drop(journal);
        retry_before_create(root).unwrap();
        assert!(!stage.exists());
        let stage = fixture();
        mark_recoverable(&stage).unwrap();
        assert!(perform(&stage).is_err());
        abandon_current(&stage).unwrap();
        assert!(stage.exists());
    }
    #[test]
    fn maintenance_handoff_pending_msi_survives_actual_next_stage_creation() {
        let fixture = fixture();
        let root = fixture.join("stages");
        std::fs::create_dir(&root).unwrap();
        let package_stage = super::super::create_download_stage_in(&root).unwrap();
        let mut writer = create_stage_file(&package_stage, "package.msi").unwrap();
        writer.write_all(b"verified MSI payload fixture").unwrap();
        let expected = writer.seal().unwrap();
        drop(writer);
        let helper = super::super::create_download_stage_in(&root).unwrap();
        assert!(package_stage.join("package.msi").exists());
        assert_eq!(
            crate::maintenance_runtime::observe_file(&package_stage.join("package.msi"), "package")
                .unwrap()
                .identity,
            expected
        );
        cleanup_download_stage(
            &package_stage,
            &package_stage.join("package.msi"),
            &expected,
        )
        .unwrap();
        assert!(!package_stage.exists());
        abandon_current(&helper).unwrap();
    }
    #[test]
    fn maintenance_handoff_cleanup_removes_only_captured_payload_and_empty_stage() {
        let stage = fixture();
        let payload = stage.join("package.zip");
        let mut writer = create_stage_file(&stage, "package.zip").unwrap();
        writer.write_all(b"verified package fixture").unwrap();
        let expected = writer.seal().unwrap();
        drop(writer);
        cleanup_download_stage(&stage, &payload, &expected).unwrap();
        assert!(!payload.exists());
        assert!(!stage.exists());
    }
    #[test]
    fn maintenance_handoff_cleanup_preserves_modified_or_unknown_neighbor() {
        let stage = fixture();
        let payload = stage.join("package.zip");
        let mut writer = create_stage_file(&stage, "package.zip").unwrap();
        writer.write_all(b"verified package fixture").unwrap();
        let expected = writer.seal().unwrap();
        drop(writer);
        std::fs::write(&payload, b"changed externally").unwrap();
        assert!(cleanup_download_stage(&stage, &payload, &expected).is_err());
        assert_eq!(std::fs::read(&payload).unwrap(), b"changed externally");
        assert!(stage.join(JOURNAL).exists());
        let stage = fixture();
        let payload = stage.join("package.zip");
        let mut writer = create_stage_file(&stage, "package.zip").unwrap();
        writer.write_all(b"verified package fixture").unwrap();
        let expected = writer.seal().unwrap();
        drop(writer);
        std::fs::write(stage.join("notes.md"), b"unknown neighbor").unwrap();
        assert!(cleanup_download_stage(&stage, &payload, &expected).is_err());
        assert_eq!(
            std::fs::read(stage.join("notes.md")).unwrap(),
            b"unknown neighbor"
        );
    }
    #[test]
    fn maintenance_handoff_cleanup_rejects_paths_outside_recorded_stage() {
        let stage = fixture();
        let outside = stage
            .parent()
            .unwrap()
            .join(format!("outside-{}", uuid::Uuid::new_v4().simple()));
        std::fs::write(&outside, b"retained").unwrap();
        let identity = crate::maintenance_runtime::observe_file(&outside, "outside")
            .unwrap()
            .identity;
        assert!(cleanup_download_stage(&stage, &outside, &identity).is_err());
        assert_eq!(std::fs::read(&outside).unwrap(), b"retained");
        assert!(studyapp_desktop::maintenance::absolute_components(&stage).is_ok());
    }
}

#[cfg(all(test, windows))]
mod early_failure_tests {
    use super::*;
    fn root() -> PathBuf {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .join(".scratch/installation-20261026/test-temp")
            .join(format!("early-stage-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }
    #[test]
    fn maintenance_handoff_17_interrupted_attempts_leave_future_stage_available() {
        let root = root();
        for attempt in 0..17 {
            retry_before_create(&root).unwrap();
            let stage = root.join(format!("{PREFIX}{}", uuid::Uuid::new_v4().simple()));
            create_private_directory(&stage).unwrap();
            {
                let _cleanup = cleanup_stage_on_error(&stage).unwrap();
                if attempt % 2 == 1 {
                    let mut writer = create_stage_file(&stage, "package.zip").unwrap();
                    writer.write_all(b"partial network response").unwrap();
                }
            }
            assert!(
                !stage.exists(),
                "failed attempt left its owned stage behind"
            );
        }
        retry_before_create(&root).unwrap();
    }
    #[test]
    fn maintenance_handoff_interrupted_cleanup_preserves_unknown_replacement() {
        let root = root();
        let stage = root.join(format!("{PREFIX}{}", uuid::Uuid::new_v4().simple()));
        create_private_directory(&stage).unwrap();
        let cleanup = cleanup_stage_on_error(&stage).unwrap();
        {
            let mut writer = create_stage_file(&stage, "package.zip").unwrap();
            writer.write_all(b"owned partial").unwrap();
        }
        std::fs::rename(
            stage.join("package.zip"),
            stage.join("retained-original.zip"),
        )
        .unwrap();
        std::fs::write(stage.join("package.zip"), b"unknown replacement").unwrap();
        drop(cleanup);
        assert_eq!(
            std::fs::read(stage.join("package.zip")).unwrap(),
            b"unknown replacement"
        );
        assert_eq!(
            std::fs::read(stage.join("retained-original.zip")).unwrap(),
            b"owned partial"
        );
        retry_before_create(&root).unwrap();
    }
    #[test]
    fn maintenance_handoff_tampered_records_cannot_authorize_cleanup_or_exhaust_quota() {
        let root = root();
        for _ in 0..17 {
            let stage = root.join(format!("{PREFIX}{}", uuid::Uuid::new_v4().simple()));
            create_private_directory(&stage).unwrap();
            let cleanup = cleanup_stage_on_error(&stage).unwrap();
            {
                let mut writer = create_stage_file(&stage, "package.zip").unwrap();
                writer.write_all(b"owned bytes now retained").unwrap();
            }
            std::fs::write(stage.join(JOURNAL), br#"{"schema":2,"files":[]}"#).unwrap();
            drop(cleanup);
            assert_eq!(
                std::fs::read(stage.join("package.zip")).unwrap(),
                b"owned bytes now retained"
            );
            retry_before_create(&root).unwrap();
        }
    }
    #[test]
    fn maintenance_handoff_failed_cleanup_retries_after_outer_directory_lease_releases() {
        let root = root();
        let stage = root.join(format!("{PREFIX}{}", uuid::Uuid::new_v4().simple()));
        create_private_directory(&stage).unwrap();
        let outer = crate::storage_backend::guard_directories(&stage, false).unwrap();
        {
            let _cleanup = cleanup_stage_on_error(&stage).unwrap();
            let mut writer = create_stage_file(&stage, "package.zip").unwrap();
            writer.write_all(b"partial").unwrap();
        }
        assert!(stage.join(JOURNAL).exists());
        assert!(!stage.join("package.zip").exists());
        drop(outer);
        retry_before_create(&root).unwrap();
        assert!(!stage.exists());
    }
}
