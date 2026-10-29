//! Official-release updater. Blocking entry points must run on a background worker.
use reqwest::{Url, blocking::Client, header};
use semver::Version;
use serde::{Deserialize, Serialize};
#[cfg(test)]
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, UNIX_EPOCH},
};
use studyapp_desktop::maintenance::PayloadKind;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn release(version: &str) -> Value {
        let name = format!("WW-StudyApp-{version}-windows-x86_64.zip");
        let url =
            format!("https://github.com/xwwpy/WW-StudyAPP/releases/download/v{version}/{name}");
        serde_json::json!({
            "tag_name": format!("v{version}"), "draft": false, "prerelease": false,
            "body": "Release notes", "html_url": format!("https://github.com/xwwpy/WW-StudyAPP/releases/tag/v{version}"),
            "assets": [
                {"name": name, "browser_download_url": url, "state": "uploaded", "size": 1024, "digest": format!("sha256:{}", "a".repeat(64))},
                {"name": format!("{name}.sha256"), "browser_download_url": format!("{url}.sha256"), "state": "uploaded", "size": 112, "digest": format!("sha256:{}", "b".repeat(64))}
            ]
        })
    }

    #[test]
    fn update_release_uses_semver_and_never_downgrades() {
        assert!(matches!(
            release_from_json(&release("0.10.0").to_string(), "0.9.0").unwrap(),
            UpdateCheck::Available(_)
        ));
        for version in ["0.6.0", "0.5.9"] {
            assert!(matches!(
                release_from_json(&release(version).to_string(), "0.6.0").unwrap(),
                UpdateCheck::UpToDate { .. }
            ));
        }
    }

    #[test]
    fn update_release_rejects_prereleases_drafts_missing_digest_and_duplicates() {
        for field in ["draft", "prerelease"] {
            let mut value = release("0.7.0");
            value[field] = true.into();
            assert!(release_from_json(&value.to_string(), "0.6.0").is_err());
        }
        assert!(release_from_json(&release("0.7.0-beta.1").to_string(), "0.6.0").is_err());
        let mut value = release("0.7.0");
        value["assets"][0]["digest"] = Value::Null;
        assert!(release_from_json(&value.to_string(), "0.6.0").is_err());
        let mut value = release("0.7.0");
        let duplicate = value["assets"][0].clone();
        value["assets"].as_array_mut().unwrap().push(duplicate);
        assert!(release_from_json(&value.to_string(), "0.6.0").is_err());
    }

    #[test]
    fn update_release_rejects_wrong_repo_tag_size_and_asset_name() {
        for bad in [
            "https://github.com/attacker/StudyApp/releases/download/v0.7.0/WW-StudyApp-0.7.0-windows-x86_64.zip",
            "https://github.com/xwwpy/WW-StudyAPP/releases/download/v0.6.0/WW-StudyApp-0.7.0-windows-x86_64.zip",
        ] {
            let mut value = release("0.7.0");
            value["assets"][0]["browser_download_url"] = bad.into();
            assert!(release_from_json(&value.to_string(), "0.6.0").is_err());
        }
        for size in [0, MAX_PACKAGE_BYTES + 1] {
            let mut value = release("0.7.0");
            value["assets"][0]["size"] = size.into();
            assert!(release_from_json(&value.to_string(), "0.6.0").is_err());
        }
        let mut value = release("0.7.0");
        value["assets"][0]["name"] = "wrong.zip".into();
        assert!(release_from_json(&value.to_string(), "0.6.0").is_err());
    }

    #[test]
    fn update_redirect_hosts_are_exact_https_without_credentials() {
        for good in [
            "https://release-assets.githubusercontent.com/path?signature=value",
            "https://objects.githubusercontent.com/path",
            "https://github.com/xwwpy/WW-StudyAPP/releases/download/v0.7.0/a.zip",
        ] {
            assert!(allowed_download_url(&reqwest::Url::parse(good).unwrap()));
        }
        for bad in [
            "http://release-assets.githubusercontent.com/x",
            "https://release-assets.githubusercontent.com.evil.test/x",
            "https://user:password@github.com/x",
            "https://github.com:444/x",
            "https://api.github.com/x",
            "https://example.com/x",
            "https://github.com/x#fragment",
        ] {
            assert!(
                !allowed_download_url(&reqwest::Url::parse(bad).unwrap()),
                "{bad}"
            );
        }
    }

    #[test]
    fn update_hash_requires_both_digest_and_exact_checksum_filename() {
        let hash = sha256_bytes(b"fixture zip");
        let filename = "WW-StudyApp-0.7.0-windows-x86_64.zip";
        assert!(
            verify_package_hash(&hash, &hash, &format!("{hash}  {filename}\n"), filename).is_ok()
        );
        assert!(
            verify_package_hash(
                &hash,
                &"0".repeat(64),
                &format!("{hash}  {filename}\n"),
                filename
            )
            .is_err()
        );
        assert!(
            verify_package_hash(
                &hash,
                &hash,
                &format!("{}  {filename}", "0".repeat(64)),
                filename
            )
            .is_err()
        );
        assert!(
            verify_package_hash(&hash, &hash, &format!("{hash}  wrong.zip"), filename).is_err()
        );
        assert!(
            verify_package_hash(
                &hash,
                &hash,
                &format!("{hash}  {filename}\n{hash}  {filename}"),
                filename
            )
            .is_err()
        );
    }

    #[test]
    fn update_response_limits_enforced_without_content_length() {
        assert_eq!(read_bounded(Cursor::new(b"1234"), 4).unwrap(), b"1234");
        assert!(read_bounded(Cursor::new(b"12345"), 4).is_err());
    }

    fn release_with_kinds(version: &str) -> Value {
        let mut value = release(version);
        let assets = value["assets"].as_array_mut().unwrap();
        for suffix in ["-portable.zip", ".msi", "-Setup.exe"] {
            let name = format!("WW-StudyApp-{version}-windows-x86_64{suffix}");
            for (name, size) in [(name.clone(), 1024), (format!("{name}.sha256"), 112)] {
                assets.push(serde_json::json!({
                    "name": name, "browser_download_url": format!("https://github.com/xwwpy/WW-StudyAPP/releases/download/v{version}/{name}"),
                    "state": "uploaded", "size": size, "digest": format!("sha256:{}", "a".repeat(64))
                }));
            }
        }
        value
    }

    #[test]
    fn update_owner_payload_selects_exact_kind_independent_of_asset_order() {
        let mut value = release_with_kinds("0.9.0");
        value["assets"].as_array_mut().unwrap().reverse();
        for (kind, suffix) in [
            (PayloadKind::LegacyZip, ".zip"),
            (PayloadKind::PortableZip, "-portable.zip"),
            (PayloadKind::Msi, ".msi"),
            (PayloadKind::Setup, "-Setup.exe"),
        ] {
            let UpdateCheck::Available(selected) =
                release_for_kind(&value.to_string(), "0.8.0", kind).unwrap()
            else {
                panic!("new release expected")
            };
            assert_eq!(selected.kind, kind);
            assert_eq!(
                selected.package.name,
                format!("WW-StudyApp-0.9.0-windows-x86_64{suffix}")
            );
        }
    }

    #[test]
    fn update_owner_payload_refuses_missing_duplicate_or_wrong_exact_checksum() {
        let base = release_with_kinds("0.9.0");
        for mode in 0..4 {
            let mut value = base.clone();
            let assets = value["assets"].as_array_mut().unwrap();
            let i = assets
                .iter()
                .position(|a| a["name"].as_str().unwrap().ends_with(".msi.sha256"))
                .unwrap();
            match mode {
                0 => {
                    assets.remove(i);
                }
                1 => {
                    assets.push(assets[i].clone());
                }
                2 => {
                    assets[i]["browser_download_url"] =
                        "https://github.com/other/repo/releases/download/v0.9.0/a.msi.sha256"
                            .into();
                }
                _ => {
                    assets[i]["state"] = "new".into();
                }
            }
            assert!(release_for_kind(&value.to_string(), "0.8.0", PayloadKind::Msi).is_err());
        }
    }

    #[test]
    fn update_owner_payload_same_version_repair_requires_exact_current_tag() {
        for candidate in ["0.7.3", "0.8.0", "0.9.0"] {
            let value = release_with_kinds(candidate);
            let result = current_tag_from_json(&value.to_string(), "0.8.0");
            assert_eq!(result.is_ok(), candidate == "0.8.0");
            if let Ok(release) = result {
                assert_eq!(release.kind, PayloadKind::PortableZip);
            }
        }
        assert!(matches!(
            release_for_kind(
                &release_with_kinds("0.8.0").to_string(),
                "0.8.0",
                PayloadKind::PortableZip
            )
            .unwrap(),
            UpdateCheck::UpToDate { .. }
        ));
        let mut wrong_tag = release_with_kinds("0.8.0");
        wrong_tag["tag_name"] = "0.8.0".into();
        assert!(current_tag_from_json(&wrong_tag.to_string(), "0.8.0").is_err());
    }

    #[cfg(feature = "ui-test")]
    #[test]
    fn repair_transport_accepts_only_exact_current_tag_routes() {
        let transport =
            RepairTransport::fixture_unchecked_for_unit_test("0.8.0", "http://127.0.0.1:32100");
        assert!(
            transport
                .route("https://api.github.com/repos/xwwpy/WW-StudyAPP/releases/tags/v0.8.0")
                .is_ok()
        );
        assert!(transport.route("https://github.com/xwwpy/WW-StudyAPP/releases/download/v0.8.0/WW-StudyApp-0.8.0-windows-x86_64-portable.zip.sha256").is_ok());
        for bad in [
            "https://api.github.com/repos/xwwpy/WW-StudyAPP/releases/tags/v0.9.0",
            "https://github.com/evil/StudyApp/releases/download/v0.8.0/WW-StudyApp-0.8.0-windows-x86_64-portable.zip",
            "http://github.com/xwwpy/WW-StudyAPP/releases/download/v0.8.0/WW-StudyApp-0.8.0-windows-x86_64-portable.zip",
        ] {
            assert!(transport.route(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn update_maintenance_status_preserves_legacy_records_and_reports_partial_results() {
        let mut status: UpdateStatus = serde_json::from_value(serde_json::json!({
            "state": "succeeded", "code": "complete", "version": "0.8.0",
            "backup_path": "", "distribution_path": "", "target_exe": "C:/app/WW-StudyApp.exe"
        }))
        .unwrap();
        assert!(status.record_path.is_empty());
        status.code = "reboot_required".into();
        assert!(status.message().contains("自行重启"));
        status.state = "failed".into();
        status.code = "partial_recovery".into();
        assert!(status.message().contains("手动恢复"));
        assert!(!status.message().contains("当前程序未替换"));
        status.code = "msi_1602".into();
        assert!(status.message().contains("安装已取消"));
    }
}

const MAX_PACKAGE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_METADATA_BYTES: u64 = 2 * 1024 * 1024;
const MAX_CHECKSUM_BYTES: u64 = 8192;
const HELPER: &str = include_str!("update_helper.ps1");

pub type UpdateResult<T> = Result<T, UpdateError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateErrorKind {
    Network,
    RateLimited,
    InvalidRelease,
    Verification,
    ManualUpdate,
    Preparation,
    InstallationChanged,
    RepairRequired,
    Launch,
}

#[derive(Clone, Debug)]
pub struct UpdateError {
    pub kind: UpdateErrorKind,
    /// Safe to show directly; raw URLs, OS errors and local paths are deliberately excluded.
    pub message: &'static str,
}

impl fmt::Display for UpdateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}
impl std::error::Error for UpdateError {}

pub(crate) fn failure(kind: UpdateErrorKind) -> UpdateError {
    let message = match kind {
        UpdateErrorKind::Network => "暂时无法连接更新服务，请检查网络后重试。",
        UpdateErrorKind::RateLimited => "更新服务暂时限制了请求，请稍后再试。",
        UpdateErrorKind::InvalidRelease => {
            "最新版本的发布信息不完整，暂时无法安全更新。请稍后重试或打开发布页面。"
        }
        UpdateErrorKind::Verification => "更新包校验未通过，当前程序未替换。请重新检查更新。",
        UpdateErrorKind::ManualUpdate => {
            "此安装位置不支持直接替换。请从官方发布页面下载 ZIP，退出程序后手动更新。"
        }
        UpdateErrorKind::Preparation => {
            "更新准备失败，当前程序未替换。请重试或从官方发布页面手动更新。"
        }
        UpdateErrorKind::InstallationChanged => {
            "程序文件在准备更新后发生了变化，已停止替换。请重新启动程序后再试。"
        }
        UpdateErrorKind::RepairRequired => "此便携版需要先在软件更新中修复安装文件，再下载更新。",
        UpdateErrorKind::Launch => "无法启动更新辅助程序，当前程序未替换。请重试或手动更新。",
    };
    UpdateError { kind, message }
}

#[derive(Clone, Debug)]
pub enum UpdateCheck {
    UpToDate { current_version: String },
    Available(Box<UpdateRelease>),
}

#[derive(Clone, Debug)]
pub struct UpdateRelease {
    pub kind: PayloadKind,
    pub version: String,
    pub tag: String,
    pub notes: String,
    pub page_url: String,
    pub package_size: u64,
    package: ReleaseAsset,
    checksum: ReleaseAsset,
}

impl UpdateRelease {
    pub fn same_artifacts(&self, other: &Self) -> bool {
        self.version == other.version
            && self.kind == other.kind
            && self.package == other.package
            && self.checksum == other.checksum
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    state: String,
    digest: Option<String>,
}

#[derive(Deserialize)]
struct ReleaseMetadata {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    body: Option<String>,
    html_url: String,
    assets: Vec<ReleaseAsset>,
}

#[derive(Clone, Debug)]
pub enum UpdateProgress {
    Downloading { downloaded: u64, total: u64 },
    Verifying,
    Preparing,
}

#[derive(Clone, Debug)]
pub struct PreparedUpdate {
    pub version: String,
    pub installation_directory: PathBuf,
    pub status_path: PathBuf,
    stage_dir: PathBuf,
    plan_sha256: String,
    maintenance: Option<crate::maintenance_execute::PreparedMaintenance>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct UpdateStatus {
    pub state: String,
    pub code: String,
    pub version: String,
    pub backup_path: String,
    pub distribution_path: String,
    #[serde(default)]
    pub record_path: String,
    target_exe: String,
}

impl UpdateStatus {
    pub fn user_message(&self) -> &'static str {
        self.message()
    }

    pub fn message(&self) -> &'static str {
        match self.state.as_str() {
            "succeeded" if self.code == "reboot_required" => {
                "更新已完成，Windows 提示需要重启。请保存其他工作后自行重启。"
            }
            "succeeded" if self.code == "cleanup_retained" => {
                "更新已完成，部分旧文件保留在原处。可查看维护记录。"
            }
            "succeeded" => "更新已完成。",
            "rolled_back" => "新版启动失败，已恢复原来的程序。请稍后重试或手动更新。",
            _ if self.code == "partial_recovery" => {
                "更新未完成，部分文件需要手动恢复。请查看维护记录并保留现有文件。"
            }
            _ if self.code == "msi_1602" => "安装已取消，原有资料库保留。",
            _ if self.code == "msi_1618" => "Windows Installer 正在处理其他安装，请稍后重试。",
            _ if self.code == "rollback_needs_help" => {
                "更新未完成。原程序备份仍保留，请打开官方发布页面进行手动恢复。"
            }
            _ => helper_failure(&self.code).message,
        }
    }
}

#[derive(Serialize)]
struct UpdatePlan {
    schema: u32,
    id: String,
    version: String,
    stage_dir: PathBuf,
    archive_path: PathBuf,
    archive_sha256: String,
    archive_size: u64,
    package_root: String,
    target_exe: PathBuf,
    old_sha256: String,
    old_size: u64,
    old_modified_ticks: String,
    parent_pid: u32,
    restart_args: Vec<String>,
    status_path: PathBuf,
}

#[derive(Clone)]
pub(crate) enum RepairTransport {
    Live,
    #[cfg(feature = "ui-test")]
    Fixture {
        version: String,
        base: Url,
        target: PathBuf,
    },
}

impl RepairTransport {
    fn route(&self, canonical: &str) -> UpdateResult<String> {
        match self {
            Self::Live => Ok(canonical.to_owned()),
            #[cfg(feature = "ui-test")]
            Self::Fixture { version, base, .. } => {
                let name = format!("WW-StudyApp-{version}-windows-x86_64-portable.zip");
                let tag = format!("v{version}");
                let route = if canonical
                    == format!("https://api.github.com/repos/xwwpy/WW-StudyAPP/releases/tags/{tag}")
                {
                    "metadata"
                } else if canonical
                    == format!(
                        "https://github.com/xwwpy/WW-StudyAPP/releases/download/{tag}/{name}.sha256"
                    )
                {
                    "checksum"
                } else if canonical
                    == format!(
                        "https://github.com/xwwpy/WW-StudyAPP/releases/download/{tag}/{name}"
                    )
                {
                    "package"
                } else {
                    return Err(failure(UpdateErrorKind::Verification));
                };
                base.join(route)
                    .map(|url| url.to_string())
                    .map_err(|_| failure(UpdateErrorKind::Verification))
            }
        }
    }

    #[cfg(feature = "ui-test")]
    pub(crate) fn fixture_for_runner(root: &Path, target: &Path, base: Url) -> UpdateResult<Self> {
        let runner =
            std::env::var_os("RUNNER_TEMP").ok_or_else(|| failure(UpdateErrorKind::Preparation))?;
        let runner = fs::canonicalize(runner).map_err(|_| failure(UpdateErrorKind::Preparation))?;
        let root = fs::canonicalize(root).map_err(|_| failure(UpdateErrorKind::Preparation))?;
        let target = fs::canonicalize(target).map_err(|_| failure(UpdateErrorKind::Preparation))?;
        if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
            || std::env::var("RUNNER_OS").as_deref() != Ok("Windows")
            || root == runner
            || !root.starts_with(&runner)
            || !target.starts_with(&root)
            || target.file_name().and_then(|name| name.to_str()) != Some("WW-StudyApp.exe")
            || base.scheme() != "http"
            || base.host_str() != Some("127.0.0.1")
            || base.port().is_none()
            || base.path() != "/"
        {
            return Err(failure(UpdateErrorKind::Preparation));
        }
        Ok(Self::Fixture {
            version: env!("CARGO_PKG_VERSION").into(),
            base,
            target,
        })
    }

    #[cfg(all(test, feature = "ui-test"))]
    fn fixture_unchecked_for_unit_test(version: &str, base: &str) -> Self {
        Self::Fixture {
            version: version.into(),
            base: Url::parse(base).unwrap(),
            target: PathBuf::new(),
        }
    }
}

fn client(timeout: Duration, transport: &RepairTransport) -> UpdateResult<Client> {
    Client::builder()
        .user_agent(concat!(
            "WW-StudyApp/",
            env!("CARGO_PKG_VERSION"),
            " (+https://github.com/xwwpy/WW-StudyAPP)"
        ))
        .https_only(matches!(transport, RepairTransport::Live))
        .connect_timeout(Duration::from_secs(10))
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| failure(UpdateErrorKind::Network))
}

fn response_ok(response: reqwest::blocking::Response) -> UpdateResult<reqwest::blocking::Response> {
    match response.status().as_u16() {
        200 => Ok(response),
        403 | 429 => Err(failure(UpdateErrorKind::RateLimited)),
        _ => Err(failure(UpdateErrorKind::Network)),
    }
}

/// Called only after the user chooses to repair this exact running version.
pub fn check_current_tag_for_repair(current_version: &str) -> UpdateResult<UpdateRelease> {
    check_current_tag_for_repair_with_transport(current_version, &RepairTransport::Live)
}

fn check_current_tag_for_repair_with_transport(
    current_version: &str,
    transport: &RepairTransport,
) -> UpdateResult<UpdateRelease> {
    let version =
        Version::parse(current_version).map_err(|_| failure(UpdateErrorKind::InvalidRelease))?;
    if !version.pre.is_empty() || !version.build.is_empty() {
        return Err(failure(UpdateErrorKind::InvalidRelease));
    }
    let api = format!("https://api.github.com/repos/xwwpy/WW-StudyAPP/releases/tags/v{version}");
    current_tag_from_json(&fetch_release_json(&api, transport)?, current_version)
}

fn fetch_release_json(api: &str, transport: &RepairTransport) -> UpdateResult<String> {
    let response = client(Duration::from_secs(30), transport)?
        .get(transport.route(api)?)
        .header(header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2026-03-10")
        .send()
        .map_err(|_| failure(UpdateErrorKind::Network))?;
    let response = response_ok(response)?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_METADATA_BYTES)
    {
        return Err(failure(UpdateErrorKind::InvalidRelease));
    }
    let bytes = read_bounded(response, MAX_METADATA_BYTES)?;
    let json = std::str::from_utf8(&bytes).map_err(|_| failure(UpdateErrorKind::InvalidRelease))?;
    Ok(json.to_owned())
}

#[cfg(feature = "ui-test")]
pub(crate) fn fixture_release_from_json(json: &str) -> UpdateResult<UpdateCheck> {
    release_from_json(json, env!("CARGO_PKG_VERSION"))
}

#[cfg(any(test, feature = "ui-test"))]
fn release_from_json(json: &str, current: &str) -> UpdateResult<UpdateCheck> {
    release_for_kind(json, current, PayloadKind::LegacyZip)
}

fn payload_name(version: &Version, kind: PayloadKind) -> String {
    let suffix = match kind {
        PayloadKind::LegacyZip => ".zip",
        PayloadKind::PortableZip => "-portable.zip",
        PayloadKind::Msi => ".msi",
        PayloadKind::Setup => "-Setup.exe",
    };
    format!("WW-StudyApp-{version}-windows-x86_64{suffix}")
}

fn release_for_kind(json: &str, current: &str, kind: PayloadKind) -> UpdateResult<UpdateCheck> {
    parse_release(json, current, kind, false)
}

fn current_tag_from_json(json: &str, current: &str) -> UpdateResult<UpdateRelease> {
    match parse_release(json, current, PayloadKind::PortableZip, true)? {
        UpdateCheck::Available(release) => Ok(*release),
        UpdateCheck::UpToDate { .. } => Err(failure(UpdateErrorKind::InvalidRelease)),
    }
}

fn parse_release(
    json: &str,
    current: &str,
    kind: PayloadKind,
    exact_current: bool,
) -> UpdateResult<UpdateCheck> {
    if json.len() as u64 > MAX_METADATA_BYTES {
        return Err(failure(UpdateErrorKind::InvalidRelease));
    }
    let metadata: ReleaseMetadata =
        serde_json::from_str(json).map_err(|_| failure(UpdateErrorKind::InvalidRelease))?;
    let version = Version::parse(
        metadata
            .tag_name
            .strip_prefix('v')
            .unwrap_or(&metadata.tag_name),
    )
    .map_err(|_| failure(UpdateErrorKind::InvalidRelease))?;
    let installed =
        Version::parse(current).map_err(|_| failure(UpdateErrorKind::InvalidRelease))?;
    if metadata.draft
        || metadata.prerelease
        || !version.pre.is_empty()
        || !version.build.is_empty()
        || metadata.tag_name != format!("v{version}")
    {
        return Err(failure(UpdateErrorKind::InvalidRelease));
    }
    if exact_current && version != installed {
        return Err(failure(UpdateErrorKind::InvalidRelease));
    }
    if !exact_current && version <= installed {
        return Ok(UpdateCheck::UpToDate {
            current_version: current.to_owned(),
        });
    }
    let page = format!(
        "https://github.com/xwwpy/WW-StudyAPP/releases/tag/{}",
        metadata.tag_name
    );
    if metadata.html_url != page || metadata.assets.len() > 64 {
        return Err(failure(UpdateErrorKind::InvalidRelease));
    }
    let name = payload_name(&version, kind);
    let select = |name: &str, limit| -> UpdateResult<ReleaseAsset> {
        let matching: Vec<_> = metadata
            .assets
            .iter()
            .filter(|asset| asset.name == name)
            .collect();
        if matching.len() != 1 {
            return Err(failure(UpdateErrorKind::InvalidRelease));
        }
        let asset = matching[0];
        validate_asset(asset, &metadata.tag_name, limit)?;
        Ok(asset.clone())
    };
    let package = select(&name, MAX_PACKAGE_BYTES)?;
    let checksum = select(&format!("{name}.sha256"), MAX_CHECKSUM_BYTES)?;
    let notes = metadata.body.unwrap_or_default();
    if notes.len() > 128 * 1024 {
        return Err(failure(UpdateErrorKind::InvalidRelease));
    }
    Ok(UpdateCheck::Available(Box::new(UpdateRelease {
        kind,
        version: version.to_string(),
        tag: metadata.tag_name,
        notes,
        page_url: page,
        package_size: package.size,
        package,
        checksum,
    })))
}

fn digest(asset: &ReleaseAsset) -> UpdateResult<String> {
    asset
        .digest
        .as_deref()
        .and_then(|value| value.strip_prefix("sha256:"))
        .filter(|value| value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| failure(UpdateErrorKind::InvalidRelease))
}

fn validate_asset(asset: &ReleaseAsset, tag: &str, limit: u64) -> UpdateResult<()> {
    let expected = format!(
        "https://github.com/xwwpy/WW-StudyAPP/releases/download/{tag}/{}",
        asset.name
    );
    if asset.browser_download_url != expected
        || asset.size == 0
        || asset.size > limit
        || asset.state != "uploaded"
    {
        return Err(failure(UpdateErrorKind::InvalidRelease));
    }
    let url = Url::parse(&expected).map_err(|_| failure(UpdateErrorKind::InvalidRelease))?;
    if !allowed_download_url(&url) || url.query().is_some() {
        return Err(failure(UpdateErrorKind::InvalidRelease));
    }
    digest(asset)?;
    Ok(())
}

fn allowed_download_url(url: &Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
        && url.port_or_known_default() == Some(443)
        && matches!(
            url.host_str(),
            Some(
                "github.com"
                    | "release-assets.githubusercontent.com"
                    | "objects.githubusercontent.com"
                    | "github-releases.githubusercontent.com"
            )
        )
}

fn download_response(
    client: &Client,
    initial: &str,
    transport: &RepairTransport,
) -> UpdateResult<reqwest::blocking::Response> {
    let mut url = Url::parse(initial).map_err(|_| failure(UpdateErrorKind::InvalidRelease))?;
    for redirects in 0..=3 {
        if !allowed_download_url(&url) {
            return Err(failure(UpdateErrorKind::Verification));
        }
        // No Authorization, Cookie or Referer is ever sent, including on a redirected request.
        let response = client
            .get(transport.route(url.as_str())?)
            .header(header::ACCEPT, "application/octet-stream")
            .send()
            .map_err(|_| failure(UpdateErrorKind::Network))?;
        if response.status().is_redirection() {
            if redirects == 3 {
                return Err(failure(UpdateErrorKind::Network));
            }
            let location = response
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| failure(UpdateErrorKind::Verification))?;
            url = url
                .join(location)
                .map_err(|_| failure(UpdateErrorKind::Verification))?;
        } else {
            return response_ok(response);
        }
    }
    Err(failure(UpdateErrorKind::Network))
}

fn read_bounded(reader: impl Read, limit: u64) -> UpdateResult<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| failure(UpdateErrorKind::Network))?;
    if bytes.len() as u64 > limit {
        return Err(failure(UpdateErrorKind::Verification));
    }
    Ok(bytes)
}

/// The signed manifest published next to the latest GitHub release.
pub(crate) const LATEST_MANIFEST_URL: &str =
    "https://github.com/xwwpy/WW-StudyAPP/releases/latest/download/latest.json";

pub(crate) fn fetch_latest_manifest_text() -> UpdateResult<String> {
    #[cfg(feature = "update-acceptance")]
    if crate::update_acceptance::active() {
        let bytes = crate::update_acceptance::fixture_bytes(LATEST_MANIFEST_URL, 262144)
            .map_err(|_| failure(UpdateErrorKind::Network))?;
        return String::from_utf8(bytes).map_err(|_| failure(UpdateErrorKind::InvalidRelease));
    }
    let transport = RepairTransport::Live;
    let client = client(Duration::from_secs(30), &transport)?;
    let response = download_response(&client, LATEST_MANIFEST_URL, &transport)?;
    let bytes = read_bounded(response, 262_144)?;
    String::from_utf8(bytes).map_err(|_| failure(UpdateErrorKind::InvalidRelease))
}

/// Streams one installer into `path`, reporting progress and refusing to grow
/// past `limit` bytes. Redirects stay inside the update download allowlist.
pub(crate) fn download_setup_to(
    url: &str,
    path: &Path,
    limit: u64,
    mut progress: impl FnMut(u64, u64),
) -> UpdateResult<()> {
    #[cfg(feature = "update-acceptance")]
    if crate::update_acceptance::active() {
        let bytes = crate::update_acceptance::fixture_bytes(url, limit)
            .map_err(|_| failure(UpdateErrorKind::Network))?;
        std::fs::write(path, &bytes).map_err(|_| failure(UpdateErrorKind::Preparation))?;
        progress(bytes.len() as u64, bytes.len() as u64);
        return Ok(());
    }
    let transport = RepairTransport::Live;
    let client = client(Duration::from_secs(600), &transport)?;
    let mut response = download_response(&client, url, &transport)?;
    let total = response.content_length().unwrap_or(0);
    let mut file = File::create(path).map_err(|_| failure(UpdateErrorKind::Preparation))?;
    let mut buffer = [0u8; 65_536];
    let mut written = 0u64;
    loop {
        let read = response
            .read(&mut buffer)
            .map_err(|_| failure(UpdateErrorKind::Network))?;
        if read == 0 {
            break;
        }
        written += read as u64;
        if written > limit {
            return Err(failure(UpdateErrorKind::Verification));
        }
        file.write_all(&buffer[..read])
            .map_err(|_| failure(UpdateErrorKind::Preparation))?;
        progress(written, total);
    }
    file.sync_all()
        .map_err(|_| failure(UpdateErrorKind::Preparation))?;
    Ok(())
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn verify_package_hash(
    actual: &str,
    api_digest: &str,
    checksum: &str,
    filename: &str,
) -> UpdateResult<()> {
    let lines: Vec<_> = checksum
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if lines.len() != 1 {
        return Err(failure(UpdateErrorKind::Verification));
    }
    let fields: Vec<_> = lines[0].split_whitespace().collect();
    if fields.len() != 2
        || fields[1].strip_prefix('*').unwrap_or(fields[1]) != filename
        || !fields[0].eq_ignore_ascii_case(actual)
        || !api_digest.eq_ignore_ascii_case(actual)
        || actual.len() != 64
        || !actual.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(failure(UpdateErrorKind::Verification));
    }
    Ok(())
}

fn hash_file(path: &Path, limit: u64) -> UpdateResult<String> {
    plain_path(path)?;
    let mut file = File::open(path).map_err(|_| failure(UpdateErrorKind::Preparation))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| failure(UpdateErrorKind::Preparation))?;
        if read == 0 {
            break;
        }
        total += read as u64;
        if total > limit {
            return Err(failure(UpdateErrorKind::Verification));
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn write_new(path: &Path, bytes: &[u8]) -> UpdateResult<()> {
    plain_path(path)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| failure(UpdateErrorKind::Preparation))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| failure(UpdateErrorKind::Preparation))
}

fn plain_path(path: &Path) -> UpdateResult<()> {
    if !path.is_absolute() || path.to_str().is_none() {
        return Err(failure(UpdateErrorKind::ManualUpdate));
    }
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                #[cfg(windows)]
                let reparse = {
                    use std::os::windows::fs::MetadataExt;
                    metadata.file_attributes() & 0x400 != 0
                };
                #[cfg(not(windows))]
                let reparse = metadata.file_type().is_symlink();
                if reparse {
                    return Err(failure(UpdateErrorKind::ManualUpdate));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(failure(UpdateErrorKind::ManualUpdate)),
        }
    }
    Ok(())
}

fn current_installation() -> UpdateResult<PathBuf> {
    let path = std::env::current_exe().map_err(|_| failure(UpdateErrorKind::ManualUpdate))?;
    // GetModuleFileName may return a verbatim prefix; the helper intentionally accepts local DOS paths only.
    let text = path
        .to_str()
        .ok_or_else(|| failure(UpdateErrorKind::ManualUpdate))?;
    let text = text.strip_prefix(r"\\?\").unwrap_or(text);
    if text.starts_with(r"\\") || text.starts_with("UNC\\") {
        return Err(failure(UpdateErrorKind::ManualUpdate));
    }
    let path = PathBuf::from(text);
    plain_path(&path)?;
    Ok(path)
}

fn update_root() -> UpdateResult<PathBuf> {
    let path = std::env::temp_dir().join("WW-StudyApp-updates");
    plain_path(&path)?;
    fs::create_dir_all(&path).map_err(|_| failure(UpdateErrorKind::Preparation))?;
    plain_path(&path)?;
    Ok(path)
}

fn status_path(root: &Path, target: &Path) -> PathBuf {
    let key = sha256_bytes(target.to_string_lossy().to_lowercase().as_bytes());
    root.join(format!("status-{key}.json"))
}

fn helper_failure(code: &str) -> UpdateError {
    failure(match code {
        "manual_update_required" | "unsupported_path" | "reparse_path" => {
            UpdateErrorKind::ManualUpdate
        }
        "installation_changed" => UpdateErrorKind::InstallationChanged,
        "archive_hash_mismatch"
        | "candidate_changed"
        | "unsafe_archive_path"
        | "archive_link"
        | "archive_limits"
        | "duplicate_archive_path"
        | "archive_path_collision"
        | "missing_distribution_file"
        | "unexpected_executable"
        | "unexpected_archive_root" => UpdateErrorKind::Verification,
        _ => UpdateErrorKind::Preparation,
    })
}

fn helper_command(stage: &Path, mode: &str) -> UpdateResult<Command> {
    #[cfg(not(windows))]
    {
        let _ = (stage, mode);
        Err(failure(UpdateErrorKind::ManualUpdate))
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let system_root =
            std::env::var_os("SystemRoot").ok_or_else(|| failure(UpdateErrorKind::Launch))?;
        let powershell =
            PathBuf::from(system_root).join("System32/WindowsPowerShell/v1.0/powershell.exe");
        plain_path(&powershell)?;
        let mut command = Command::new(powershell);
        command
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-WindowStyle",
                "Hidden",
                "-File",
            ])
            .arg(stage.join("update-helper.ps1"))
            .args(["-Mode", mode, "-PlanPath"])
            .arg(stage.join("plan.json"))
            .stdin(Stdio::null())
            .creation_flags(0x08000000);
        Ok(command)
    }
}

fn download_verified_release(
    release: &UpdateRelease,
    stage: &Path,
    progress: &impl Fn(UpdateProgress),
    transport: &RepairTransport,
) -> UpdateResult<(
    PathBuf,
    String,
    u64,
    studyapp_desktop::maintenance::FileIdentity,
)> {
    let modern = matches!(release.kind, PayloadKind::PortableZip | PayloadKind::Msi);
    let mut cleanup_on_error = modern
        .then(|| crate::maintenance_handoff::cleanup_stage_on_error(stage))
        .transpose()
        .map_err(|_| failure(UpdateErrorKind::Preparation))?;
    let network = client(Duration::from_secs(180), transport)?;
    let checksum_response =
        download_response(&network, &release.checksum.browser_download_url, transport)?;
    let checksum = read_bounded(checksum_response, MAX_CHECKSUM_BYTES)?;
    if checksum.len() as u64 != release.checksum.size
        || sha256_bytes(&checksum) != digest(&release.checksum)?
    {
        return Err(failure(UpdateErrorKind::Verification));
    }
    let checksum =
        std::str::from_utf8(&checksum).map_err(|_| failure(UpdateErrorKind::Verification))?;
    let mut response =
        download_response(&network, &release.package.browser_download_url, transport)?;
    if response
        .content_length()
        .is_some_and(|size| size != release.package.size)
    {
        return Err(failure(UpdateErrorKind::Verification));
    }
    let archive_path = stage.join(if release.kind == PayloadKind::Msi {
        "package.msi"
    } else {
        "package.zip"
    });
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ);
    }
    let mut tracked = if modern {
        Some(
            crate::maintenance_handoff::create_stage_file(
                stage,
                if release.kind == PayloadKind::Msi {
                    "package.msi"
                } else {
                    "package.zip"
                },
            )
            .map_err(|_| failure(UpdateErrorKind::Preparation))?,
        )
    } else {
        None
    };
    // The frozen schema-1 helper keeps its original staging protocol.
    let mut legacy = if modern {
        None
    } else {
        Some(
            options
                .open(&archive_path)
                .map_err(|_| failure(UpdateErrorKind::Preparation))?,
        )
    };
    let file = if let Some(file) = tracked.as_mut() {
        file.file_mut()
    } else {
        legacy
            .as_mut()
            .ok_or_else(|| failure(UpdateErrorKind::Preparation))?
    };
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut downloaded = 0u64;
    progress(UpdateProgress::Downloading {
        downloaded,
        total: release.package.size,
    });
    loop {
        let read = response
            .read(&mut buffer)
            .map_err(|_| failure(UpdateErrorKind::Network))?;
        if read == 0 {
            break;
        }
        downloaded += read as u64;
        if downloaded > release.package.size || downloaded > MAX_PACKAGE_BYTES {
            return Err(failure(UpdateErrorKind::Verification));
        }
        file.write_all(&buffer[..read])
            .map_err(|_| failure(UpdateErrorKind::Preparation))?;
        hasher.update(&buffer[..read]);
        progress(UpdateProgress::Downloading {
            downloaded,
            total: release.package.size,
        });
    }
    file.sync_all()
        .map_err(|_| failure(UpdateErrorKind::Preparation))?;
    if downloaded != release.package.size {
        return Err(failure(UpdateErrorKind::Verification));
    }
    progress(UpdateProgress::Verifying);
    let hash = format!("{:x}", hasher.finalize());
    verify_package_hash(
        &hash,
        &digest(&release.package)?,
        checksum,
        &release.package.name,
    )?;
    file.rewind()
        .map_err(|_| failure(UpdateErrorKind::Preparation))?;
    let (identity, links) = crate::maintenance_runtime::handle_identity(file, false)
        .map_err(|_| failure(UpdateErrorKind::Verification))?;
    if links != 1 || identity.sha256 != hash || identity.size != downloaded {
        return Err(failure(UpdateErrorKind::Verification));
    }
    if let Some(file) = tracked.as_mut() {
        let sealed = file
            .seal()
            .map_err(|_| failure(UpdateErrorKind::Preparation))?;
        if sealed != identity {
            return Err(failure(UpdateErrorKind::Verification));
        }
    }
    if let Some(cleanup) = cleanup_on_error.as_mut() {
        cleanup.disarm();
    }
    Ok((archive_path, hash, downloaded, identity))
}

fn cleanup_download(
    stage: &Path,
    payload: &Path,
    identity: &studyapp_desktop::maintenance::FileIdentity,
) {
    if let Err(error) = crate::maintenance_handoff::cleanup_download_stage(stage, payload, identity)
    {
        // The plan already contains independently verified bytes. A retained
        // temporary file must not turn a completed repair into a false failure.
        eprintln!(
            "WW-StudyApp: 临时安装文件已保留（{}）：{error:#}",
            stage.display()
        );
    }
}

fn maintenance_context(
    data_dir: Option<&Path>,
) -> UpdateResult<crate::maintenance_runtime::DiscoveryContext> {
    let mut context = crate::maintenance_runtime::DiscoveryContext::current_user()
        .map_err(|_| failure(UpdateErrorKind::Preparation))?;
    context.explicit_library = data_dir.map(Path::to_owned);
    Ok(context)
}

fn protected_update_paths(
    context: &crate::maintenance_runtime::DiscoveryContext,
) -> UpdateResult<studyapp_desktop::maintenance::ProtectedPaths> {
    let library = crate::maintenance_runtime::discover_library(context);
    if !library.issues.is_empty() {
        return Err(failure(UpdateErrorKind::Preparation));
    }
    let mut protected = studyapp_desktop::maintenance::ProtectedPaths::default();
    if let Some(directory) = library.database_path.as_deref().and_then(Path::parent) {
        protected.directories.push(directory.to_owned());
    }
    if context.explicit_library.is_none()
        && let Some(directory) = context.location_file.as_deref().and_then(Path::parent)
    {
        protected.directories.push(directory.to_owned());
    }
    Ok(protected)
}

/// Read-only inspection for the settings repair action. No network or adoption.
pub fn portable_repair_needed() -> UpdateResult<bool> {
    let target = current_installation()?;
    if crate::maintenance_runtime::current_payload_kind(&target)
        .map_err(|_| failure(UpdateErrorKind::InstallationChanged))?
        == PayloadKind::Msi
    {
        return Ok(false);
    }
    let context = maintenance_context(None)?;
    Ok(crate::maintenance_runtime::discover(&target, &context)
        .receipt
        .is_none())
}

/// Explicit repair only. A same-version package never appears as an available update.
pub fn repair_current_installation(
    data_dir: Option<&Path>,
    progress: impl Fn(UpdateProgress),
) -> UpdateResult<()> {
    let target = current_installation()?;
    repair_installation_core(&target, data_dir, &RepairTransport::Live, progress)
}

#[cfg(feature = "ui-test")]
pub(crate) fn repair_fixture_installation(
    target: &Path,
    data_dir: Option<&Path>,
    transport: &RepairTransport,
    progress: impl Fn(UpdateProgress),
) -> UpdateResult<()> {
    let RepairTransport::Fixture {
        target: guarded, ..
    } = transport
    else {
        return Err(failure(UpdateErrorKind::Preparation));
    };
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
        || std::env::var("RUNNER_OS").as_deref() != Ok("Windows")
        || fs::canonicalize(target).map_err(|_| failure(UpdateErrorKind::Preparation))? != *guarded
    {
        return Err(failure(UpdateErrorKind::Preparation));
    }
    repair_installation_core(target, data_dir, transport, progress)
}

fn repair_installation_core(
    target: &Path,
    data_dir: Option<&Path>,
    transport: &RepairTransport,
    progress: impl Fn(UpdateProgress),
) -> UpdateResult<()> {
    if crate::maintenance_runtime::current_payload_kind(target)
        .map_err(|_| failure(UpdateErrorKind::InstallationChanged))?
        != PayloadKind::PortableZip
    {
        return Err(failure(UpdateErrorKind::ManualUpdate));
    }
    let context = maintenance_context(data_dir)?;
    let protected = protected_update_paths(&context)?;
    let release = match transport {
        RepairTransport::Live => check_current_tag_for_repair(env!("CARGO_PKG_VERSION"))?,
        #[cfg(feature = "ui-test")]
        RepairTransport::Fixture { .. } => {
            check_current_tag_for_repair_with_transport(env!("CARGO_PKG_VERSION"), transport)?
        }
    };
    let stage = crate::maintenance_handoff::create_download_stage()
        .map_err(|_| failure(UpdateErrorKind::Preparation))?;
    let _guards = crate::storage_backend::guard_directories(&stage, false)
        .map_err(|_| failure(UpdateErrorKind::Preparation))?;
    let (archive, hash, _, identity) =
        download_verified_release(&release, &stage, &progress, transport)?;
    progress(UpdateProgress::Preparing);
    let repaired =
        crate::maintenance_execute::reconcile_current_package(&archive, &hash, target, &protected);
    drop(_guards);
    cleanup_download(&stage, &archive, &identity);
    repaired.map_err(|_| failure(UpdateErrorKind::InstallationChanged))?;
    Ok(())
}

/// Explicit download only, with registered MSI ownership taking precedence.
pub fn prepare_update(
    release: &UpdateRelease,
    data_dir: Option<&Path>,
    progress: impl Fn(UpdateProgress),
) -> UpdateResult<PreparedUpdate> {
    let target = current_installation()?;
    let kind = crate::maintenance_runtime::current_payload_kind(&target)
        .map_err(|_| failure(UpdateErrorKind::InstallationChanged))?;
    // Retain the original helper contract for compatibility fixtures; normal
    // owner-aware selection always returns PortableZip or Msi.
    if release.kind == PayloadKind::LegacyZip {
        if kind == PayloadKind::Msi {
            return Err(failure(UpdateErrorKind::InstallationChanged));
        }
        return prepare_legacy_update(release, data_dir, progress);
    }
    let installed = Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|_| failure(UpdateErrorKind::InvalidRelease))?;
    let next =
        Version::parse(&release.version).map_err(|_| failure(UpdateErrorKind::InvalidRelease))?;
    if release.kind != kind
        || next <= installed
        || !next.pre.is_empty()
        || !next.build.is_empty()
        || release.tag != format!("v{next}")
        || release.package.name != payload_name(&next, kind)
        || release.checksum.name != format!("{}.sha256", release.package.name)
    {
        return Err(failure(UpdateErrorKind::InvalidRelease));
    }
    validate_asset(&release.package, &release.tag, MAX_PACKAGE_BYTES)?;
    validate_asset(&release.checksum, &release.tag, MAX_CHECKSUM_BYTES)?;
    let context = maintenance_context(data_dir)?;
    let protected = protected_update_paths(&context)?;
    let receipt = if kind == PayloadKind::Msi {
        crate::maintenance_runtime::discover(&target, &context)
            .receipt
            .ok_or_else(|| failure(UpdateErrorKind::InstallationChanged))?
    } else {
        crate::maintenance_runtime::read_portable_receipt(&target)
            .map_err(|_| failure(UpdateErrorKind::RepairRequired))?
    };
    let stage = crate::maintenance_handoff::create_download_stage()
        .map_err(|_| failure(UpdateErrorKind::Preparation))?;
    let _guards = crate::storage_backend::guard_directories(&stage, false)
        .map_err(|_| failure(UpdateErrorKind::Preparation))?;
    let (payload, hash, _, identity) =
        download_verified_release(release, &stage, &progress, &RepairTransport::Live)?;
    progress(UpdateProgress::Preparing);
    let prepared = match kind {
        PayloadKind::PortableZip => crate::maintenance_execute::prepare_portable_update(
            &payload,
            &hash,
            &receipt,
            &protected,
            &release.version,
        ),
        PayloadKind::Msi => crate::maintenance_execute::prepare_msi_update(
            &payload,
            &hash,
            &receipt,
            &release.version,
        ),
        _ => return Err(failure(UpdateErrorKind::InvalidRelease)),
    };
    drop(_guards);
    cleanup_download(&stage, &payload, &identity);
    let mut maintenance = prepared.map_err(|_| failure(UpdateErrorKind::Preparation))?;
    maintenance
        .with_restart_data_directory(data_dir)
        .map_err(|_| failure(UpdateErrorKind::Preparation))?;
    Ok(PreparedUpdate {
        version: release.version.clone(),
        installation_directory: maintenance.install_root().to_owned(),
        status_path: maintenance.status_path().to_owned(),
        stage_dir: stage,
        plan_sha256: String::new(),
        maintenance: Some(maintenance),
    })
}

/// Explicit update click only. Downloads and validates in a unique temporary directory.
/// `data_dir` should be the explicit/effective library selection that must survive restart.
fn prepare_legacy_update(
    release: &UpdateRelease,
    data_dir: Option<&Path>,
    progress: impl Fn(UpdateProgress),
) -> UpdateResult<PreparedUpdate> {
    if !cfg!(windows) {
        return Err(failure(UpdateErrorKind::ManualUpdate));
    }
    let installed = Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|_| failure(UpdateErrorKind::InvalidRelease))?;
    let next =
        Version::parse(&release.version).map_err(|_| failure(UpdateErrorKind::InvalidRelease))?;
    if next <= installed
        || !next.pre.is_empty()
        || !next.build.is_empty()
        || !matches!(release.tag.as_str(), tag if tag == next.to_string() || tag == format!("v{next}"))
        || release.package.name != format!("WW-StudyApp-{next}-windows-x86_64.zip")
        || release.checksum.name != format!("{}.sha256", release.package.name)
    {
        return Err(failure(UpdateErrorKind::InvalidRelease));
    }
    validate_asset(&release.package, &release.tag, MAX_PACKAGE_BYTES)?;
    validate_asset(&release.checksum, &release.tag, MAX_CHECKSUM_BYTES)?;
    let target = current_installation()?;
    let metadata = fs::metadata(&target).map_err(|_| failure(UpdateErrorKind::ManualUpdate))?;
    let old_hash = hash_file(&target, 512 * 1024 * 1024)?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .ok_or_else(|| failure(UpdateErrorKind::ManualUpdate))?;
    let ticks = (modified.as_nanos() / 100 + 621_355_968_000_000_000).to_string();
    let root = update_root()?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    let stage = root.join(&id);
    fs::create_dir(&stage).map_err(|_| failure(UpdateErrorKind::Preparation))?;
    plain_path(&stage)?;
    let (archive_path, hash, downloaded, _) =
        download_verified_release(release, &stage, &progress, &RepairTransport::Live)?;
    let mut restart_args = Vec::new();
    if let Some(directory) = data_dir {
        let directory =
            std::path::absolute(directory).map_err(|_| failure(UpdateErrorKind::Preparation))?;
        restart_args = vec![
            "--data-dir".into(),
            directory
                .to_str()
                .ok_or_else(|| failure(UpdateErrorKind::Preparation))?
                .to_owned(),
        ];
    }
    let status = status_path(&root, &target);
    plain_path(&status)?;
    let plan = UpdatePlan {
        schema: 1,
        id,
        version: release.version.clone(),
        stage_dir: stage.clone(),
        archive_path,
        archive_sha256: hash,
        archive_size: downloaded,
        package_root: format!("WW-StudyApp-{}-windows-x86_64", release.version),
        target_exe: target.clone(),
        old_sha256: old_hash,
        old_size: metadata.len(),
        old_modified_ticks: ticks,
        parent_pid: std::process::id(),
        restart_args,
        status_path: status.clone(),
    };
    let plan_bytes =
        serde_json::to_vec(&plan).map_err(|_| failure(UpdateErrorKind::Preparation))?;
    write_new(&stage.join("plan.json"), &plan_bytes)?;
    write_new(&stage.join("update-helper.ps1"), HELPER.as_bytes())?;
    progress(UpdateProgress::Preparing);
    let output = helper_command(&stage, "Prepare")?
        .output()
        .map_err(|_| failure(UpdateErrorKind::Launch))?;
    if !output.status.success() {
        return Err(helper_failure(
            String::from_utf8_lossy(&output.stderr).trim(),
        ));
    }
    Ok(PreparedUpdate {
        version: release.version.clone(),
        installation_directory: target.parent().unwrap_or(&target).to_owned(),
        status_path: status,
        stage_dir: stage,
        plan_sha256: sha256_bytes(&plan_bytes),
        maintenance: None,
    })
}

/// Call only after drafts/settings are durable. On success the caller must quit normally.
pub fn launch_prepared_update(prepared: &PreparedUpdate) -> UpdateResult<()> {
    if let Some(maintenance) = &prepared.maintenance {
        return crate::maintenance_handoff::begin_update_handoff(maintenance)
            .map_err(|_| failure(UpdateErrorKind::Launch));
    }
    plain_path(&prepared.stage_dir)?;
    if hash_file(&prepared.stage_dir.join("plan.json"), 64 * 1024)? != prepared.plan_sha256
        || hash_file(&prepared.stage_dir.join("update-helper.ps1"), 1024 * 1024)?
            != sha256_bytes(HELPER.as_bytes())
    {
        return Err(failure(UpdateErrorKind::Verification));
    }
    helper_command(&prepared.stage_dir, "Apply")?
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| failure(UpdateErrorKind::Launch))?;
    Ok(())
}

/// Read install-specific completion status after startup. This never modifies the installation.
pub fn read_last_update_status() -> UpdateResult<Option<UpdateStatus>> {
    let target = current_installation()?;
    if let Some(status) = crate::maintenance_execute::read_last_maintenance_status(&target) {
        use crate::maintenance_execute::MaintenanceState;
        let state = match status.state {
            MaintenanceState::Succeeded => "succeeded",
            MaintenanceState::RolledBack => "rolled_back",
            MaintenanceState::Failed => "failed",
            _ => return Ok(None),
        };
        let record = target
            .parent()
            .ok_or_else(|| failure(UpdateErrorKind::Preparation))?
            .join(format!(
                ".studyapp-maintenance-{}/status.json",
                status.transaction_id
            ));
        return Ok(Some(UpdateStatus {
            state: state.into(),
            code: status.code,
            version: status.version,
            backup_path: String::new(),
            distribution_path: String::new(),
            record_path: record.to_string_lossy().into_owned(),
            target_exe: target.to_string_lossy().into_owned(),
        }));
    }
    let root = std::env::temp_dir().join("WW-StudyApp-updates");
    let path = status_path(&root, &target);
    plain_path(&path)?;
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(failure(UpdateErrorKind::Preparation)),
    };
    let value: UpdateStatus = serde_json::from_slice(&read_bounded(file, 64 * 1024)?)
        .map_err(|_| failure(UpdateErrorKind::Preparation))?;
    if !value
        .target_exe
        .eq_ignore_ascii_case(&target.to_string_lossy())
    {
        return Err(failure(UpdateErrorKind::Verification));
    }
    if value.state == "prepared" {
        return Ok(None);
    }
    Ok(Some(value))
}
