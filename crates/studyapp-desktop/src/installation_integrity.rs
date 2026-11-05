//! Read-only file comparison against the current release's signed inventory.
use anyhow::{Context, Result, ensure};
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

const MAX_FILE: u64 = 256 * 1024 * 1024;
const GUIDES: &[&str] = &[
    "object-storage.md",
    "release-notes.md",
    "storage.md",
    "sync-guide.md",
    "updates.md",
    "user-guide.md",
];
const IMAGES: &[&str] = &[
    "dashboard.png",
    "managed-images.png",
    "note-editor.png",
    "sync-settings.png",
    "workspace.png",
];

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Baseline {
    schema: u32,
    version: String,
    platform: String,
    package_sha256: String,
    files: Vec<BaselineFile>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BaselineFile {
    path: String,
    size: u64,
    sha256: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum FileState {
    Missing,
    Changed,
    Unreadable,
}
impl FileState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Missing => "缺失",
            Self::Changed => "内容不符",
            Self::Unreadable => "无法安全读取",
        }
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct Finding {
    pub path: String,
    pub state: FileState,
}
#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub version: String,
    pub platform: String,
    pub checked: usize,
    pub findings: Vec<Finding>,
}
impl Report {
    pub fn summary(&self) -> String {
        if self.findings.is_empty() {
            format!(
                "已检查 {} 个发行文件，内容与当前版本的签名基线一致。",
                self.checked
            )
        } else {
            format!(
                "已检查 {} 个发行文件，发现 {} 项缺失、内容不符或无法读取。",
                self.checked,
                self.findings.len()
            )
        }
    }
}

fn allowed_paths(platform: &str) -> Result<BTreeSet<String>> {
    let files: Vec<String> = match platform {
        "windows-x86_64" => [
            "WW-StudyApp.exe",
            "vcruntime140.dll",
            "vcruntime140_1.dll",
            "README.md",
            "LICENSE",
            "RELEASE-NOTES.md",
            "THIRD-PARTY-NOTICES.txt",
        ]
        .map(str::to_owned)
        .into(),
        "linux-x86_64" => {
            let mut files = vec![
                "bin/studyapp".into(),
                "share/applications/studyapp.desktop".into(),
                "share/icons/hicolor/512x512/apps/studyapp.png".into(),
                "share/doc/studyapp/copyright".into(),
                "share/doc/studyapp/README.md".into(),
            ];
            files.extend(
                GUIDES
                    .iter()
                    .map(|name| format!("share/doc/studyapp/docs/{name}")),
            );
            files.extend(
                IMAGES
                    .iter()
                    .map(|name| format!("share/doc/studyapp/docs/images/{name}")),
            );
            files
        }
        "macos-arm64" | "macos-x86_64" => {
            let mut files = vec![
                "MacOS/studyapp".into(),
                "Info.plist".into(),
                "_CodeSignature/CodeResources".into(),
                "Resources/studyapp.icns".into(),
                "Resources/LICENSE".into(),
                "Resources/README.md".into(),
            ];
            files.extend(GUIDES.iter().map(|name| format!("Resources/docs/{name}")));
            files.extend(
                IMAGES
                    .iter()
                    .map(|name| format!("Resources/docs/images/{name}")),
            );
            files
        }
        _ => anyhow::bail!("不支持的平台"),
    };
    Ok(files.into_iter().collect())
}
fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn baseline_from_signed(
    text: &str,
    version: &str,
    platform: &str,
    key: &[u8; 32],
) -> Result<Baseline> {
    let manifest =
        crate::update_manifest::parse_with_key(text, platform, key).map_err(anyhow::Error::msg)?;
    ensure!(
        manifest.version.to_string() == version,
        "发行基线不是当前安装版本"
    );
    let envelope: serde_json::Value = serde_json::from_str(text)?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(
        envelope["signed_payload"]
            .as_str()
            .context("缺少签名内容")?,
    )?;
    let body: serde_json::Value = serde_json::from_slice(&bytes)?;
    // Never consume the unauthenticated, compatibility fields of the envelope.
    let baseline: Baseline = serde_json::from_value(body["integrity"][platform].clone())
        .context("当前版本没有可信文件基线")?;
    ensure!(
        baseline.schema == 1
            && baseline.version == version
            && baseline.platform == platform
            && baseline.package_sha256 == manifest.sha256,
        "发行基线身份无效"
    );
    let allowed = allowed_paths(platform)?;
    let mut seen = BTreeSet::new();
    ensure!(baseline.files.len() == allowed.len(), "发行基线不完整");
    for file in &baseline.files {
        ensure!(
            allowed.contains(&file.path)
                && seen.insert(file.path.clone())
                && file.size > 0
                && file.size <= MAX_FILE
                && valid_hash(&file.sha256),
            "发行基线文件无效"
        );
    }
    Ok(baseline)
}

fn root_for(executable: &Path, platform: &str) -> Result<PathBuf> {
    let parent = executable.parent().context("无法定位程序目录")?;
    let root = match platform {
        "windows-x86_64" => parent,
        "linux-x86_64" if parent.file_name().is_some_and(|name| name == "bin") => {
            parent.parent().context("无法定位发行目录")?
        }
        "macos-arm64" | "macos-x86_64"
            if parent.file_name().is_some_and(|name| name == "MacOS") =>
        {
            let contents = parent.parent().context("无法定位应用包")?;
            ensure!(
                contents.file_name().is_some_and(|name| name == "Contents"),
                "不支持的应用包位置"
            );
            contents
        }
        _ => anyhow::bail!("当前目录不是支持的发行布局"),
    };
    Ok(root.to_owned())
}
fn single_link(file: &File) -> Result<bool> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };
        let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
        // SAFETY: the borrowed file handle and correctly typed output remain live.
        ensure!(
            unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) } != 0,
            "无法读取文件身份"
        );
        // SAFETY: the successful call initialized the output.
        Ok(unsafe { info.assume_init() }.nNumberOfLinks == 1)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(file.metadata()?.nlink() == 1)
    }
}
fn compare_file(root: &Path, expected: &BaselineFile) -> Result<Option<FileState>> {
    let path = root.join(&expected.path);
    let _guards =
        crate::storage_backend::guard_directories(path.parent().context("路径无效")?, false)?;
    let file = crate::storage_backend::plain_handle(&path, false, true)?;
    ensure!(single_link(&file)?, "不检查多链接文件");
    let before = file.metadata()?;
    if before.len() != expected.size {
        return Ok(Some(FileState::Changed));
    }
    let mut hash = Sha256::new();
    let mut reader = (&file).take(expected.size + 1);
    let mut buffer = [0u8; 65536];
    let mut count = 0u64;
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        count += n as u64;
        hash.update(&buffer[..n]);
    }
    let after = file.metadata()?;
    ensure!(
        before.len() == after.len() && before.modified()? == after.modified()?,
        "检查时文件发生变化"
    );
    Ok(
        (count != expected.size || format!("{:x}", hash.finalize()) != expected.sha256)
            .then_some(FileState::Changed),
    )
}
fn inspect(root: &Path, baseline: &Baseline) -> Result<Report> {
    let _guards = crate::storage_backend::guard_directories(root, false)?;
    let mut report = Report {
        version: baseline.version.clone(),
        platform: baseline.platform.clone(),
        checked: 0,
        findings: vec![],
    };
    for expected in &baseline.files {
        let state = match compare_file(root, expected) {
            Ok(state) => state,
            Err(error) => Some(
                if error.chain().any(|cause| {
                    cause
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
                }) {
                    FileState::Missing
                } else {
                    FileState::Unreadable
                },
            ),
        };
        report.checked += 1;
        if let Some(state) = state {
            report.findings.push(Finding {
                path: expected.path.clone(),
                state,
            });
        }
    }
    Ok(report)
}

pub fn check_current() -> Result<Report, String> {
    let platform = crate::update_manifest::current_platform()?;
    let version = env!("CARGO_PKG_VERSION");
    let text = crate::update_backend::fetch_current_manifest_text(version).map_err(|_| {
        "无法取得当前版本的发行基线，请检查网络后重试。此次未判断文件是否完整。".to_owned()
    })?;
    let baseline = baseline_from_signed(
        &text,
        version,
        platform,
        crate::update_signature::verification_key(),
    )
    .map_err(|_| {
        "当前版本没有可验证的文件基线，或基线校验未通过。此次未判断文件是否完整。".to_owned()
    })?;
    let executable = std::env::current_exe().map_err(|_| "无法定位当前程序；未检查文件。")?;
    let root =
        root_for(&executable, platform).map_err(|_| "当前目录不是支持的发行布局；未检查文件。")?;
    inspect(&root, &baseline).map_err(|_| "无法安全读取程序目录；未判断文件是否完整。".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use serde_json::json;
    fn fixture() -> (PathBuf, Baseline) {
        let root = tempfile::tempdir().unwrap().keep();
        let files = allowed_paths("windows-x86_64")
            .unwrap()
            .into_iter()
            .map(|path| {
                let bytes = format!("original {path}").into_bytes();
                std::fs::write(root.join(&path), &bytes).unwrap();
                BaselineFile {
                    path,
                    size: bytes.len() as u64,
                    sha256: format!("{:x}", Sha256::digest(bytes)),
                }
            })
            .collect();
        std::fs::write(root.join("private-notes.sqlite3"), b"private unchanged").unwrap();
        (
            root,
            Baseline {
                schema: 1,
                version: "0.3.0".into(),
                platform: "windows-x86_64".into(),
                package_sha256: "e".repeat(64),
                files,
            },
        )
    }
    fn signed(baseline: &Baseline) -> String {
        let key = SigningKey::from_bytes(&[17; 32]);
        let body = json!({"schema":1,"version":"0.3.0","notes":"current","integrity":{"windows-x86_64":baseline},"platforms":{"windows-x86_64":{"url":"https://github.com/xwwpy/WW-StudyAPP/releases/download/v0.3.0/WW-StudyApp-0.3.0-windows-x86_64-Setup.exe","signature":"fixture","size":123,"sha256":"e".repeat(64)}}});
        let bytes = serde_json::to_vec(&body).unwrap();
        let mut message = b"WW-StudyApp update manifest v1\0".to_vec();
        message.extend_from_slice(&bytes);
        json!({"signed_payload":base64::engine::general_purpose::STANDARD.encode(bytes),"manifest_signature":base64::engine::general_purpose::STANDARD.encode(key.sign(&message).to_bytes()),"integrity":{"windows-x86_64":{"files":[{"path":"private-notes.sqlite3"}]}}}).to_string()
    }
    fn read(text: &str) -> Result<Baseline> {
        baseline_from_signed(
            text,
            "0.3.0",
            "windows-x86_64",
            &SigningKey::from_bytes(&[17; 32]).verifying_key().to_bytes(),
        )
    }
    fn snapshot(root: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
        std::fs::read_dir(root)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                let bytes = std::fs::read(&path).unwrap();
                (path, bytes)
            })
            .collect()
    }
    #[test]
    fn integrity_signed_baseline_ignores_unsigned_envelope_and_rejects_tampering() {
        let (_, baseline) = fixture();
        assert_eq!(read(&signed(&baseline)).unwrap().files.len(), 7);
        let mut bad: serde_json::Value = serde_json::from_str(&signed(&baseline)).unwrap();
        bad["manifest_signature"] = json!("AAAA");
        assert!(read(&bad.to_string()).is_err());
        let mut bad = baseline.clone();
        bad.version = "0.3.1".into();
        assert!(read(&signed(&bad)).is_err());
        let mut bad = baseline.clone();
        bad.platform = "macos-arm64".into();
        assert!(read(&signed(&bad)).is_err());
        let mut bad = baseline.clone();
        bad.package_sha256 = "f".repeat(64);
        assert!(read(&signed(&bad)).is_err());
        for path in [
            "../private",
            "private-notes.sqlite3",
            "support/install-receipt.json",
            "C:/private",
        ] {
            let mut bad = baseline.clone();
            bad.files[0].path = path.into();
            assert!(read(&signed(&bad)).is_err());
        }
        let mut bad = baseline.clone();
        bad.files.pop();
        assert!(read(&signed(&bad)).is_err());
        let mut bad = baseline.clone();
        bad.files[1] = bad.files[0].clone();
        assert!(read(&signed(&bad)).is_err());
    }
    #[test]
    fn integrity_reports_missing_and_changed_without_repairing_or_writing_receipts() {
        let (root, baseline) = fixture();
        let before = snapshot(&root);
        assert!(inspect(&root, &baseline).unwrap().findings.is_empty());
        assert_eq!(before, snapshot(&root));
        std::fs::rename(
            root.join("vcruntime140.dll"),
            root.join("vcruntime140.dll.missing"),
        )
        .unwrap();
        std::fs::write(root.join("WW-StudyApp.exe"), b"corrupt").unwrap();
        let before = snapshot(&root);
        let report = inspect(&root, &baseline).unwrap();
        assert_eq!(report.checked, 7);
        assert_eq!(report.findings.len(), 2);
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.path == "vcruntime140.dll" && f.state == FileState::Missing)
        );
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.path == "WW-StudyApp.exe" && f.state == FileState::Changed)
        );
        assert_eq!(before, snapshot(&root));
        assert!(!root.join("support").exists());
    }
    #[test]
    fn integrity_rejects_hard_links_without_reading_or_changing_the_neighbor() {
        let (root, baseline) = fixture();
        std::fs::rename(root.join("README.md"), root.join("original-readme")).unwrap();
        std::fs::hard_link(root.join("original-readme"), root.join("README.md")).unwrap();
        let before = snapshot(&root);
        let report = inspect(&root, &baseline).unwrap();
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.path == "README.md" && f.state == FileState::Unreadable)
        );
        assert_eq!(before, snapshot(&root));
    }
    #[cfg(unix)]
    #[test]
    fn integrity_does_not_follow_a_symbolic_link_to_private_data() {
        let (root, baseline) = fixture();
        std::fs::rename(root.join("README.md"), root.join("original-readme")).unwrap();
        std::os::unix::fs::symlink(root.join("private-notes.sqlite3"), root.join("README.md"))
            .unwrap();
        let report = inspect(&root, &baseline).unwrap();
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.path == "README.md" && f.state == FileState::Unreadable)
        );
        assert_eq!(
            std::fs::read(root.join("private-notes.sqlite3")).unwrap(),
            b"private unchanged"
        );
    }
    #[test]
    fn integrity_roots_are_derived_from_actual_platform_layout_only() {
        let root = PathBuf::from(if cfg!(windows) {
            "C:/fixture"
        } else {
            "/fixture"
        });
        assert_eq!(
            root_for(&root.join("studyapp.exe"), "windows-x86_64").unwrap(),
            root
        );
        assert_eq!(
            root_for(&root.join("bin/studyapp"), "linux-x86_64").unwrap(),
            root
        );
        assert_eq!(
            root_for(&root.join("WW.app/Contents/MacOS/studyapp"), "macos-arm64").unwrap(),
            root.join("WW.app/Contents")
        );
        assert!(root_for(&root.join("random/studyapp"), "linux-x86_64").is_err());
        assert!(root_for(&root.join("Other/MacOS/studyapp"), "macos-arm64").is_err());
    }
}
