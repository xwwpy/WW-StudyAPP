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
#[derive(Clone, Debug, Default)]
pub struct ApplicationReport {
    pub logical_bytes: u64,
    pub distinct_file_bytes: u64,
    pub files: u64,
    pub locations: Vec<ApplicationLocation>,
    pub issues: Vec<ApplicationIssue>,
    pub incomplete: bool,
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
}
#[derive(Clone, Debug)]
pub struct ApplicationIssue {
    pub path: PathBuf,
    pub message: String,
}
fn location(label: &str, path: PathBuf, movable: bool) -> ApplicationLocation {
    ApplicationLocation {
        file_only: false,
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
                report.issues.push(ApplicationIssue {
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
                        report.issues.push(ApplicationIssue {
                            path: e.path(),
                            message: format!("{error:#}"),
                        });
                    }
                }
                Err(error) => {
                    seen.entries += 1;
                    report.issues.push(ApplicationIssue {
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
    let mut report = ApplicationReport {
        issues,
        ..Default::default()
    };
    let mut seen = Seen {
        budget,
        ..Seen::default()
    };
    for mut row in locations {
        let path = row.path.clone();
        if !seen.exhausted
            && let Err(error) = studyapp_desktop::app_paths::validate_path(&path)
                .and_then(|_| visit(&path, 0, &mut row, &mut report, &mut seen))
        {
            report.issues.push(ApplicationIssue {
                path: row.path.clone(),
                message: format!("{error:#}"),
            });
        }
        report.locations.push(row);
    }
    report.incomplete = !report.issues.is_empty();
    report
}
pub fn inspect(
    paths: &AppPaths,
    library: &Path,
    external: impl IntoIterator<Item = PathBuf>,
    native: bool,
) -> ApplicationReport {
    let mut locations = vec![location("当前资料库", library.into(), true)];
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
    for path in external {
        let mut row = location("已登记外置图片", path, false);
        row.file_only = true;
        locations.push(row);
    }
    let mut issues = vec![];
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
        let report = inspect(&paths, &root.join("library"), [indexed], false);
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
