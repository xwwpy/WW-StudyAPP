//! Read-only union inventory. File lengths are not allocated disk blocks.
use anyhow::{Result, ensure};
use std::{
    collections::HashSet,
    fs::File,
    path::{Path, PathBuf},
};
use studyapp_desktop::app_paths::AppPaths;
/// Resolve binary runtime paths without consulting real profiles in CLI unit tests.
pub(crate) fn runtime_paths() -> Result<AppPaths> {
    #[cfg(feature = "update-acceptance")]
    if let Some(fixture) = crate::update_acceptance::launch_directory() {
        return acceptance_runtime_paths(&fixture);
    }
    if cfg!(test) {
        let root =
            std::env::temp_dir().join(format!("studyapp-runtime-test-{}", std::process::id()));
        let dirs = studyapp_desktop::app_paths::AppDirectories {
            data: root.join("library"),
            config: root.join("config"),
            local_data: root.join("local"),
            cache: root.join("cache"),
            updates: root.join("updates"),
        };
        AppPaths::from_defaults(dirs, root.join("config/storage-location.json"))
    } else {
        AppPaths::runtime()
    }
}
#[cfg(feature = "update-acceptance")]
fn acceptance_runtime_paths(fixture: &Path) -> Result<AppPaths> {
    // The fixture is transported in the authenticated launch context on macOS,
    // and the explicit runner environment elsewhere. Its identity outlives PIDs.
    let root = std::fs::canonicalize(fixture)?.join(".runtime");
    let dirs = studyapp_desktop::app_paths::AppDirectories {
        data: root.join("library"),
        config: root.join("config"),
        local_data: root.join("local"),
        cache: root.join("cache"),
        updates: root.join("updates"),
    };
    AppPaths::from_defaults(dirs, root.join("config/storage-location.json"))
}
#[cfg(all(test, feature = "update-acceptance"))]
mod acceptance_paths_tests {
    use super::*;
    #[test]
    fn relaunch_child_reads_original_transaction_pointer() {
        let Some(probe) = std::env::var_os("STUDYAPP_PATH_PROBE_FILE") else {
            return;
        };
        let paths = runtime_paths().unwrap();
        let pointer = paths.update_pointer().unwrap();
        std::fs::write(
            probe,
            serde_json::to_vec(&serde_json::json!({
                "pid":std::process::id(), "pointer":pointer,
                "value":std::fs::read_to_string(pointer).unwrap(),
                "profile":paths.editor_profile(),
            }))
            .unwrap(),
        )
        .unwrap();
    }
    #[test]
    fn acceptance_process_restarts_share_isolated_paths_and_status_evidence() {
        let fixture = tempfile::tempdir().unwrap();
        std::fs::write(fixture.path().join("fixture.json"), b"{}").unwrap();
        let paths = acceptance_runtime_paths(fixture.path()).unwrap();
        let pointer = paths.update_pointer().unwrap();
        std::fs::create_dir_all(pointer.parent().unwrap()).unwrap();
        std::fs::write(&pointer, "original-denied-transaction").unwrap();
        let mut observed = Vec::new();
        for index in 0..2 {
            let probe = fixture.path().join(format!("probe-{index}.json"));
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "app_usage::acceptance_paths_tests::relaunch_child_reads_original_transaction_pointer"])
                .env("GITHUB_ACTIONS", "true")
                .env("STUDYAPP_UPDATE_ACCEPTANCE_DIR", fixture.path())
                .env("STUDYAPP_PATH_PROBE_FILE", &probe).output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let value: serde_json::Value =
                serde_json::from_slice(&std::fs::read(probe).unwrap()).unwrap();
            assert_eq!(value["pointer"], serde_json::json!(pointer));
            assert_eq!(value["profile"], serde_json::json!(paths.editor_profile()));
            assert_eq!(value["value"], "original-denied-transaction");
            observed.push(value);
        }
        assert_ne!(observed[0]["pid"], observed[1]["pid"]);
        assert!(pointer.starts_with(std::fs::canonicalize(fixture.path()).unwrap()));
    }
}
#[derive(Clone, Debug, Default)]
pub struct ApplicationReport {
    pub logical_bytes: u64,
    pub distinct_file_bytes: u64,
    pub files: u64,
    pub locations: Vec<ApplicationLocation>,
    pub issues: Vec<ApplicationIssue>,
    pub incomplete: bool,
}
impl ApplicationReport {
    fn record_issue(&mut self, issue: ApplicationIssue) {
        if self.issues.len() < 128 {
            self.issues.push(issue);
        } else if self.issues.len() == 128 {
            self.issues.push(ApplicationIssue {
                path: issue.path,
                message: "还有其他位置无法统计，未逐一列出。".into(),
            });
        }
    }
}
#[derive(Clone, Debug)]
pub struct ApplicationLocation {
    pub label: String,
    pub path: PathBuf,
    pub bytes: u64,
    pub files: u64,
    pub movable: bool,
    pub reason: String,
    file_only: bool,
    members: Vec<PathBuf>,
}
#[derive(Clone, Debug)]
pub struct ApplicationIssue {
    pub path: PathBuf,
    pub message: String,
}
fn location(label: &str, path: PathBuf, movable: bool) -> ApplicationLocation {
    ApplicationLocation {
        file_only: false,
        members: Vec::new(),
        label: label.into(),
        path,
        bytes: 0,
        files: 0,
        movable,
        reason: if movable {
            "支持迁移；当前仅统计"
        } else {
            "由安装器或系统管理"
        }
        .into(),
    }
}
#[derive(Default)]
struct Seen {
    paths: HashSet<PathBuf>,
    identities: HashSet<(u64, u128)>,
    entries: usize,
    budget: usize,
    exhausted: bool,
}
pub(crate) fn identity(file: &File) -> Result<(u64, u128)> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };
        let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
        // SAFETY: file owns the live handle and output has the API's required layout.
        ensure!(
            unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) } != 0,
            "无法读取文件身份"
        );
        // SAFETY: API success initialized the output.
        let info = unsafe { info.assume_init() };
        Ok((
            u64::from(info.dwVolumeSerialNumber),
            (u128::from(info.nFileIndexHigh) << 32) | u128::from(info.nFileIndexLow),
        ))
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = file.metadata()?;
        Ok((m.dev(), u128::from(m.ino())))
    }
}
fn visit(
    path: &Path,
    depth: usize,
    row: &mut ApplicationLocation,
    report: &mut ApplicationReport,
    seen: &mut Seen,
) -> Result<()> {
    if seen.exhausted {
        return Ok(());
    }
    if seen.entries >= seen.budget {
        seen.exhausted = true;
        anyhow::bail!("统计文件数量超过上限");
    }
    ensure!(depth <= 128, "统计深度超过上限");
    seen.entries += 1;
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    let _parents = crate::storage_backend::guard_directories(path.parent().unwrap_or(path), false)?;
    let directory = metadata.is_dir();
    ensure!(
        !row.file_only || metadata.is_file(),
        "登记的外置图片不是普通文件"
    );
    let file = crate::storage_backend::plain_handle(path, directory, false)?;
    let canonical = std::fs::canonicalize(path)?;
    if !seen.paths.insert(canonical) {
        return Ok(());
    }
    if directory {
        let mut entries = std::fs::read_dir(path)?;
        loop {
            if seen.exhausted {
                break;
            }
            // Check before advancing ReadDir, including siblings whose inspection fails.
            if seen.entries >= seen.budget {
                seen.exhausted = true;
                report.record_issue(ApplicationIssue {
                    path: path.into(),
                    message: "统计文件数量超过上限".into(),
                });
                break;
            }
            let Some(entry) = entries.next() else {
                break;
            };
            match entry {
                Ok(e) => {
                    if let Err(error) = visit(&e.path(), depth + 1, row, report, seen) {
                        report.record_issue(ApplicationIssue {
                            path: e.path(),
                            message: format!("{error:#}"),
                        });
                    }
                }
                Err(error) => {
                    seen.entries += 1;
                    report.record_issue(ApplicationIssue {
                        path: path.into(),
                        message: error.to_string(),
                    });
                }
            }
        }
    } else {
        let size = file.metadata()?.len();
        row.bytes = row.bytes.saturating_add(size);
        row.files += 1;
        report.logical_bytes = report.logical_bytes.saturating_add(size);
        let id = identity(&file)?;
        if seen.identities.insert(id) {
            report.distinct_file_bytes = report.distinct_file_bytes.saturating_add(size);
            report.files += 1;
        }
    }
    Ok(())
}
pub fn scan(
    locations: Vec<ApplicationLocation>,
    issues: Vec<ApplicationIssue>,
) -> ApplicationReport {
    scan_with_budget(locations, issues, 1_000_000)
}
fn scan_with_budget(
    locations: Vec<ApplicationLocation>,
    issues: Vec<ApplicationIssue>,
    budget: usize,
) -> ApplicationReport {
    let mut report = ApplicationReport::default();
    for issue in issues {
        report.record_issue(issue);
    }
    let mut seen = Seen {
        budget,
        ..Seen::default()
    };
    for mut row in locations {
        let path = row.path.clone();
        let members = if row.members.is_empty() {
            vec![path]
        } else {
            std::mem::take(&mut row.members)
        };
        for file in members {
            if seen.exhausted {
                break;
            }
            if let Err(error) = studyapp_desktop::app_paths::validate_path(&file)
                .and_then(|_| visit(&file, 0, &mut row, &mut report, &mut seen))
            {
                report.record_issue(ApplicationIssue {
                    path: file,
                    message: format!("{error:#}"),
                });
            }
        }
        report.locations.push(row);
    }
    report.incomplete = !report.issues.is_empty();
    report
}
#[cfg(test)]
pub fn inspect(
    paths: &AppPaths,
    library: &Path,
    external: impl IntoIterator<Item = PathBuf>,
    native: bool,
    migration: Result<(Option<PathBuf>, Vec<PathBuf>)>,
) -> ApplicationReport {
    inspect_with_logging(
        paths,
        library,
        external,
        native,
        migration,
        &Default::default(),
    )
}
pub fn inspect_with_logging(
    paths: &AppPaths,
    library: &Path,
    external: impl IntoIterator<Item = PathBuf>,
    native: bool,
    migration: Result<(Option<PathBuf>, Vec<PathBuf>)>,
    logging: &studyapp_core::LoggingSettings,
) -> ApplicationReport {
    let directory = crate::audit_log::configuration(logging)
        .map(|config| config.directory())
        .unwrap_or_else(|| {
            logging
                .directory
                .as_ref()
                .map(PathBuf::from)
                .unwrap_or_else(|| paths.effective.local_data.join("logs"))
        });
    let mut locations = Vec::new();
    let mut issues = Vec::new();
    match crate::audit_log::inventory(&directory) {
        Ok(files) if !files.is_empty() => {
            let mut logs = location("本地日志", directory, false);
            logs.file_only = true;
            logs.members = files;
            logs.reason =
                "仅统计已登记的日志、所有权记录和写入锁；不扫描自定义目录的其他文件".into();
            locations.push(logs);
        }
        Ok(_) => {}
        Err(_) => issues.push(ApplicationIssue {
            path: directory,
            message: "日志目录所有权或访问权限无法核对；未扫描其中的其他文件。".into(),
        }),
    }
    locations.push(location("当前资料库", library.into(), true));
    for (prefix, d) in [("当前", &paths.effective), ("默认/遗留", &paths.defaults)] {
        for (label, path) in [
            ("资料", &d.data),
            ("配置", &d.config),
            ("本地数据", &d.local_data),
            ("编辑器缓存", &d.cache),
            ("更新", &d.updates),
        ] {
            locations.push(location(&format!("{prefix}{label}"), path.clone(), true));
        }
    }
    let portable = paths.default_editor_profile();
    if !portable.starts_with(&paths.defaults.cache) {
        let mut row = location("便携版编辑器缓存", portable, paths.custom_root.is_none());
        row.reason = "便携启动器登记的编辑器文件".into();
        locations.push(row);
    }
    // On Windows ProjectDirs keeps separate data/config/cache subdirectories
    // under two dedicated StudyApp/StudyApp roots. Also count sibling files.
    #[cfg(windows)]
    for parent in [&paths.defaults.data, &paths.defaults.local_data]
        .into_iter()
        .filter_map(|p| p.parent())
    {
        if parent.ends_with(Path::new("StudyApp").join("StudyApp")) {
            let mut row = location("应用目录内其他文件", parent.into(), false);
            row.reason = "包含应用专用目录内未单独分类的文件".into();
            locations.push(row);
        }
    }
    let mut image_groups = std::collections::BTreeMap::<PathBuf, Vec<PathBuf>>::new();
    for file in external {
        if let Some(parent) = file.parent() {
            image_groups
                .entry(parent.parent().unwrap_or(parent).to_owned())
                .or_default()
                .push(file);
        }
    }
    for (root, files) in image_groups {
        let mut row = location("已登记外置图片", root, false);
        row.file_only = true;
        row.members = files;
        row.reason = "仅统计图片索引登记的文件".into();
        locations.push(row);
    }
    match migration {
        Ok((Some(stage), files)) => {
            let mut tx = location("迁移记录", stage.clone(), false);
            tx.reason = "可恢复迁移所用的位置".into();
            locations.push(tx);
            if !files.is_empty() {
                let mut old = location("迁移保留的旧文件", stage.join("cleanup.json"), false);
                old.reason = "只统计已登记且未重复计入的文件，可在应用位置中查看清单".into();
                old.file_only = true;
                old.members = files;
                locations.push(old);
            }
        }
        Err(e) => issues.push(ApplicationIssue {
            path: paths.locator.clone(),
            message: format!("无法完整核对迁移文件：{e:#}"),
        }),
        Ok((None, _)) => {}
    }
    if native {
        discover_runtime(&mut locations, &mut issues);
    }
    scan(locations, issues)
}
fn discover_runtime(locations: &mut Vec<ApplicationLocation>, issues: &mut Vec<ApplicationIssue>) {
    match std::env::current_exe() {
        Ok(exe) => {
            let verified = installation_locations(&exe, locations);
            if let Err(error) = verified {
                locations.push(location("程序安装文件", exe.clone(), false));
                issues.push(ApplicationIssue {
                    path: exe,
                    message: format!("仅统计当前可执行文件；安装清单未验证：{error:#}"),
                });
            }
        }
        Err(error) => issues.push(ApplicationIssue {
            path: PathBuf::new(),
            message: format!("无法定位程序：{error}"),
        }),
    }
    let temp = std::env::temp_dir();
    match std::fs::read_dir(&temp) {
        Ok(entries) => {
            for (index, entry) in entries.take(100_001).enumerate() {
                if index == 100_000 {
                    issues.push(ApplicationIssue {
                        path: temp.clone(),
                        message: "临时目录条目超出统计上限".into(),
                    });
                    break;
                }
                match entry {
                    Ok(entry) => {
                        let name = entry.file_name();
                        let name = name.to_string_lossy();
                        if [
                            "studyapp-update-",
                            "studyapp-install-",
                            "WW-StudyApp-maintenance-",
                            "WW-StudyApp-Setup-",
                        ]
                        .iter()
                        .any(|p| name.starts_with(p))
                        {
                            let path = entry.path();
                            if (name.starts_with("studyapp-update-")
                                || name.starts_with("studyapp-install-"))
                                && verified_package(&path).is_ok()
                            {
                                locations.push(location("已验证更新包", path, false));
                            } else {
                                issues.push(ApplicationIssue {
                                    path,
                                    message:
                                        "暂存目录所有权或活动事务尚未验证；未递归统计，不可迁移"
                                            .into(),
                                });
                            }
                        }
                    }
                    Err(e) => issues.push(ApplicationIssue {
                        path: temp.clone(),
                        message: e.to_string(),
                    }),
                }
            }
        }
        Err(e) => issues.push(ApplicationIssue {
            path: temp,
            message: e.to_string(),
        }),
    }
    #[cfg(target_os = "linux")]
    locations.push(location(
        "系统更新暂存",
        PathBuf::from("/var/lib/studyapp-updates"),
        false,
    ));
    #[cfg(target_os = "macos")]
    if let Some(base) = directories::BaseDirs::new() {
        for suffix in [
            "Library/WebKit/io.github.xwwpy.studyapp",
            "Library/Caches/io.github.xwwpy.studyapp",
            "Library/Containers/io.github.xwwpy.studyapp/Data/Library/WebKit",
        ] {
            let path = base.home_dir().join(suffix);
            locations.push(location("系统管理的 WebKit 候选目录", path.clone(), false));
            issues.push(ApplicationIssue {
                path,
                message: "系统管理；候选范围尚待 macOS 原生 CI 验证，覆盖不完整且不可任意迁移"
                    .into(),
            });
        }
    }
}

fn read_stage_file(path: &Path, limit: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    let _guards = crate::storage_backend::guard_directories(
        path.parent()
            .ok_or_else(|| anyhow::anyhow!("暂存文件目录无效"))?,
        false,
    )?;
    let file = crate::storage_backend::plain_handle(path, false, true)?;
    ensure!(file.metadata()?.len() <= limit, "暂存文件超过大小限制");
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= limit, "读取中的暂存文件超过大小限制");
    Ok(bytes)
}
fn verified_package(path: &Path) -> Result<()> {
    use sha2::{Digest, Sha256};
    let _guards = crate::storage_backend::guard_directories(path, false)?;
    let envelope = String::from_utf8(read_stage_file(
        &path.join("manifest.json"),
        2 * 1024 * 1024,
    )?)?;
    let manifest =
        crate::update_manifest::parse_latest_json(&envelope).map_err(anyhow::Error::msg)?;
    ensure!(manifest.size <= 256 * 1024 * 1024, "更新包超过大小限制");
    let name = if cfg!(windows) {
        "package.exe"
    } else {
        "package"
    };
    let bytes = read_stage_file(&path.join(name), manifest.size)?;
    ensure!(
        bytes.len() as u64 == manifest.size
            && format!("{:x}", Sha256::digest(&bytes)) == manifest.sha256,
        "更新包长度或摘要不匹配"
    );
    crate::update_signature::verify_manifest_signature(&bytes, &manifest.signature)
        .map_err(anyhow::Error::msg)?;
    Ok(())
}
fn installation_locations(exe: &Path, locations: &mut Vec<ApplicationLocation>) -> Result<()> {
    #[cfg(windows)]
    {
        // The discovery adapter verifies ownership, root identity and the exact receipt payload.
        let context = crate::maintenance_runtime::DiscoveryContext {
            app_paths: None,
            default_library: PathBuf::new(),
            location_file: None,
            explicit_library: None,
        };
        let discovery = crate::maintenance_runtime::discover(exe, &context);
        let receipt = discovery
            .receipt
            .ok_or_else(|| anyhow::anyhow!("安装收据或 MSI 所有权无法验证"))?;
        for file in receipt.files {
            locations.push(location(
                "程序安装文件",
                receipt.install_root.join(file.relative_path),
                false,
            ));
        }
        locations.push(location(
            "安装收据",
            receipt
                .install_root
                .join(studyapp_desktop::maintenance::RECEIPT_PATH),
            false,
        ));
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        ensure!(
            exe == Path::new("/usr/bin/studyapp"),
            "开发或非标准安装路径缺少包清单"
        );
        for path in [
            "/usr/bin/studyapp",
            "/usr/share/applications/studyapp.desktop",
            "/usr/share/icons/hicolor/512x512/apps/studyapp.png",
            "/usr/share/doc/studyapp",
        ] {
            locations.push(location("程序安装文件", PathBuf::from(path), false));
        }
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        let bundle = exe
            .ancestors()
            .find(|p| p.extension().is_some_and(|e| e == "app"))
            .ok_or_else(|| anyhow::anyhow!("没有已验证的应用包"))?;
        let plist = bundle.join("Contents/Info.plist");
        let _guards = crate::storage_backend::guard_directories(plist.parent().unwrap(), false)?;
        let file = crate::storage_backend::plain_handle(&plist, false, true)?;
        ensure!(file.metadata()?.len() <= 65536, "应用包元数据过大");
        use std::io::Read;
        let mut text = String::new();
        file.take(65537).read_to_string(&mut text)?;
        ensure!(
            text.split_once("<key>CFBundleIdentifier</key>")
                .is_some_and(|(_, tail)| tail
                    .trim_start()
                    .starts_with("<string>io.github.xwwpy.studyapp</string>")),
            "应用包标识未验证"
        );
        locations.push(location("程序应用包", bundle.into(), false));
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn root() -> PathBuf {
        tempfile::tempdir().unwrap().keep()
    }
    #[test]
    fn overlapping_roots_and_hardlinks_count_once() {
        let p = root();
        std::fs::create_dir(p.join("library")).unwrap();
        std::fs::write(p.join("library/a"), b"abc").unwrap();
        std::fs::hard_link(p.join("library/a"), p.join("alias")).unwrap();
        std::fs::write(p.join("cache"), b"12345").unwrap();
        let r = scan(
            vec![
                location("library", p.join("library"), true),
                location("data", p.clone(), true),
                location("config", p.clone(), true),
            ],
            vec![],
        );
        assert_eq!(r.logical_bytes, 11);
        assert_eq!(r.distinct_file_bytes, 8);
        assert_eq!(r.files, 2);
        assert!(!r.incomplete);
    }
    #[test]
    fn missing_is_empty_and_unsafe_is_incomplete() {
        let p = root();
        let r = scan(vec![location("missing", p.join("missing"), true)], vec![]);
        assert_eq!(r.logical_bytes, 0);
        assert!(!r.incomplete);
        let r = scan(
            vec![location("invalid", PathBuf::from("relative"), true)],
            vec![],
        );
        assert!(r.incomplete);
        assert!(!r.issues.is_empty());
    }
    #[cfg(unix)]
    #[test]
    fn links_are_not_followed() {
        let p = root();
        let other = root();
        std::fs::write(other.join("secret"), b"hidden").unwrap();
        std::os::unix::fs::symlink(other, p.join("link")).unwrap();
        let r = scan(vec![location("data", p, true)], vec![]);
        assert_eq!(r.logical_bytes, 0);
        assert!(r.incomplete);
    }
}

#[cfg(test)]
mod safety_tests {
    use super::*;
    #[test]
    fn migration_usage_counts_only_registered_source_files_and_transaction_data() {
        let root = tempfile::tempdir().unwrap().keep();
        let dirs = studyapp_desktop::app_paths::AppDirectories {
            data: root.join("library"),
            config: root.join("config"),
            local_data: root.join("local"),
            cache: root.join("cache"),
            updates: root.join("updates"),
        };
        let paths = studyapp_desktop::app_paths::AppPaths::from_defaults(
            dirs,
            root.join("config/location.json"),
        )
        .unwrap();
        let old = root.join("old");
        let stage = root.join("transaction");
        std::fs::create_dir(&old).unwrap();
        std::fs::create_dir(&stage).unwrap();
        std::fs::write(old.join("registered"), b"note").unwrap();
        std::fs::write(old.join("unrelated"), b"user content").unwrap();
        std::fs::write(stage.join("cleanup.json"), b"123").unwrap();
        let report = inspect(
            &paths,
            &root.join("library"),
            [],
            false,
            Ok((Some(stage), vec![old.join("registered")])),
        );
        assert!(!report.incomplete);
        assert_eq!(report.logical_bytes, 7);
        assert_eq!(report.distinct_file_bytes, 7);
    }
    #[test]
    fn log_inventory_counts_only_owned_external_files_and_union_deduplicates_nested_roots() {
        let root = tempfile::tempdir().unwrap();
        let dirs = studyapp_desktop::app_paths::AppDirectories {
            data: root.path().join("data"),
            config: root.path().join("config"),
            local_data: root.path().join("local"),
            cache: root.path().join("cache"),
            updates: root.path().join("updates"),
        };
        let paths = AppPaths::from_defaults(dirs, root.path().join("location.json")).unwrap();
        for directory in [
            root.path().join("external"),
            paths.effective.local_data.join("logs"),
        ] {
            let config = crate::audit_log::Configuration {
                settings: Default::default(),
                default_directory: directory.clone(),
            };
            let runtime = crate::audit_log::Runtime::start(Some(config)).unwrap();
            runtime.logger.record(crate::audit_log::Record::new(
                studyapp_core::LogLevel::Info,
                crate::audit_log::Phase::ApplicationStart,
                crate::audit_log::Outcome::Success,
            ));
            assert!(runtime.logger.flush(std::time::Duration::from_secs(5)));
            let owned = crate::audit_log::inventory(&directory).unwrap();
            let expected: u64 = owned
                .iter()
                .map(|path| std::fs::metadata(path).unwrap().len())
                .sum();
            // External unrelated content must never be registered or traversed.
            let unrelated = root.path().join("external/unrelated");
            if directory.ends_with("external") {
                std::fs::write(&unrelated, b"private unrelated file").unwrap();
            }
            let settings = studyapp_core::LoggingSettings {
                directory: Some(directory.to_string_lossy().into_owned()),
                ..Default::default()
            };
            let report = inspect_with_logging(
                &paths,
                &root.path().join("library"),
                [],
                false,
                Ok((None, vec![])),
                &settings,
            );
            let logs = report
                .locations
                .iter()
                .find(|row| row.label == "本地日志")
                .unwrap();
            assert_eq!(logs.bytes, expected);
            assert_eq!(logs.files, owned.len() as u64);
            assert!(!report.incomplete);
            // When nested in local_data, the same owned paths occur in both rows,
            // but the union still includes every byte exactly once.
            assert_eq!(report.logical_bytes, expected);
            assert_eq!(report.distinct_file_bytes, expected);
            assert_eq!(report.files, owned.len() as u64);
        }
    }
    #[test]
    fn unverified_log_directory_reports_incomplete_instead_of_scanning_user_files() {
        let root = tempfile::tempdir().unwrap();
        let external = root.path().join("external");
        std::fs::create_dir(&external).unwrap();
        std::fs::write(external.join("user-file"), b"private").unwrap();
        let dirs = studyapp_desktop::app_paths::AppDirectories {
            data: root.path().join("data"),
            config: root.path().join("config"),
            local_data: root.path().join("local"),
            cache: root.path().join("cache"),
            updates: root.path().join("updates"),
        };
        let paths = AppPaths::from_defaults(dirs, root.path().join("location.json")).unwrap();
        let settings = studyapp_core::LoggingSettings {
            directory: Some(external.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let report = inspect_with_logging(
            &paths,
            &root.path().join("library"),
            [],
            false,
            Ok((None, vec![])),
            &settings,
        );
        assert!(report.incomplete);
        assert_eq!(report.logical_bytes, 0);
        assert_eq!(report.files, 0);
    }
    #[test]
    fn indexed_external_file_does_not_scan_sibling() {
        let root = tempfile::tempdir().unwrap().keep();
        std::fs::write(root.join("indexed"), b"123").unwrap();
        std::fs::write(root.join("unrelated"), b"123456789").unwrap();
        let report = scan(
            vec![location("external", root.join("indexed"), false)],
            vec![],
        );
        assert_eq!(report.logical_bytes, 3);
    }
    #[test]
    fn indexed_directory_is_never_traversed() {
        let root = tempfile::tempdir().unwrap().keep();
        let dirs = studyapp_desktop::app_paths::AppDirectories {
            data: root.join("data"),
            config: root.join("config"),
            local_data: root.join("local"),
            cache: root.join("cache"),
            updates: root.join("updates"),
        };
        let paths = AppPaths::from_defaults(dirs, root.join("locator.json")).unwrap();
        let indexed = root.join("external/old-image.png");
        std::fs::create_dir_all(&indexed).unwrap();
        std::fs::write(indexed.join("unrelated"), b"secret").unwrap();
        let report = inspect(
            &paths,
            &root.join("library"),
            [indexed],
            false,
            Ok((None, vec![])),
        );
        assert_eq!(report.logical_bytes, 0);
        assert!(report.incomplete);
    }
    #[test]
    fn traversal_budget_stops_siblings_and_bounds_diagnostics() {
        let root = tempfile::tempdir().unwrap().keep();
        for n in 0..40 {
            std::fs::write(root.join(n.to_string()), b"x").unwrap();
        }
        let report = scan_with_budget(vec![location("root", root, true)], vec![], 4);
        assert_eq!(report.files, 3);
        assert_eq!(report.issues.len(), 1);
        assert!(report.incomplete);
    }
    #[cfg(windows)]
    #[test]
    fn junction_and_access_denied_are_incomplete_without_following() {
        let root = tempfile::tempdir().unwrap().keep();
        let outside = tempfile::tempdir().unwrap().keep();
        std::fs::write(outside.join("secret"), b"secret").unwrap();
        let output = std::process::Command::new("cmd.exe")
            .args(["/c", "mklink", "/J"])
            .arg(root.join("junction"))
            .arg(&outside)
            .output()
            .unwrap();
        assert!(output.status.success());
        let report = scan(vec![location("root", root.clone(), true)], vec![]);
        assert_eq!(report.logical_bytes, 0);
        assert!(report.incomplete);
        assert!(read_stage_file(&root.join("junction/secret"), 64).is_err());
        use std::os::windows::fs::OpenOptionsExt;
        let protected = root.join("locked");
        std::fs::write(&protected, b"locked").unwrap();
        let _lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&protected)
            .unwrap();
        let report = scan(vec![location("locked", protected.clone(), true)], vec![]);
        assert!(report.incomplete);
        assert!(report.issues.iter().any(|i| i.path == protected));
    }
}

#[cfg(test)]
mod stage_tests {
    use super::*;
    #[test]
    fn stage_file_reads_are_bounded_and_guarded() {
        let root = tempfile::tempdir().unwrap().keep();
        let path = root.join("manifest.json");
        std::fs::write(&path, b"12345").unwrap();
        assert!(read_stage_file(&path, 4).is_err());
        assert_eq!(read_stage_file(&path, 5).unwrap(), b"12345");
    }
}
