//! Strict modern archive reader. No archive-supplied path is used for extraction.
use super::*;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub(super) struct VerifiedArchive {
    pub manifest: PackageManifest,
    pub members: BTreeMap<String, Vec<u8>>,
}
pub(super) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub(super) fn read_archive(path: &Path, expected: &str) -> Result<VerifiedArchive> {
    ensure!(valid_sha256(expected), "归档校验无效");
    let _guards =
        crate::storage_backend::guard_directories(path.parent().context("归档目录无效")?, false)?;
    let mut handle = crate::storage_backend::plain_handle(path, false, true)?;
    let (identity, links) = runtime::handle_identity(&handle, false)?;
    ensure!(
        links == 1 && identity.size <= 256 * 1024 * 1024 && identity.sha256 == expected,
        "归档校验失败"
    );
    use std::io::Seek;
    handle.rewind()?;
    let mut zip = zip::ZipArchive::new(handle)?;
    ensure!(zip.len() == 7, "归档资源数量无效");
    let mut full = BTreeMap::new();
    let mut total = 0u64;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        ensure!(
            !entry.is_dir() && !entry.encrypted(),
            "不支持目录或加密资源"
        );
        ensure!(
            entry
                .unix_mode()
                .is_none_or(|mode| mode & 0o170000 == 0 || mode & 0o170000 == 0o100000),
            "归档不能包含链接"
        );
        ensure!(
            entry.size() > 0 && entry.size() <= 256 * 1024 * 1024,
            "归档资源大小无效"
        );
        total = total.checked_add(entry.size()).context("归档大小溢出")?;
        ensure!(total <= 512 * 1024 * 1024, "归档总大小超过限制");
        let name = entry.name()?.into_owned();
        normalize_relative_path(&name)?;
        ensure!(!full.contains_key(&name), "归档资源重复");
        let size = entry.size();
        let mut bytes = Vec::new();
        entry.by_ref().take(size + 1).read_to_end(&mut bytes)?;
        ensure!(bytes.len() as u64 == size, "归档资源长度不符");
        full.insert(name, bytes);
    }
    let manifest_key = full
        .keys()
        .find(|name| name.ends_with("/support/manifest.json"))
        .context("缺少资源清单")?
        .clone();
    let bytes = &full[&manifest_key];
    ensure!(bytes.len() <= 1024 * 1024, "资源清单过大");
    let mut manifest: PackageManifest = serde_json::from_slice(bytes)?;
    validate_manifest(&manifest)?;
    let version = semver::Version::parse(&manifest.version)?;
    ensure!(
        version.pre.is_empty() && version.build.is_empty(),
        "资源版本无效"
    );
    let prefix = format!("WW-StudyApp-{}-windows-x86_64-portable/", manifest.version);
    let allowed = [
        ("WW-StudyApp.exe", FileRole::Application),
        ("Uninstall.exe", FileRole::Uninstaller),
        ("vcruntime140.dll", FileRole::Runtime),
        ("vcruntime140_1.dll", FileRole::Runtime),
        ("support/LICENSE", FileRole::Notice),
        ("support/THIRD-PARTY-NOTICES.txt", FileRole::Notice),
    ];
    ensure!(manifest.files.len() == allowed.len(), "资源清单数量无效");
    let mut members = BTreeMap::new();
    for (name, role) in allowed {
        let record = manifest
            .files
            .iter()
            .find(|f| f.relative_path == name)
            .context("资源清单缺少必需文件")?;
        ensure!(record.role == role, "资源角色无效");
        let bytes = full
            .remove(&format!("{prefix}{name}"))
            .context("归档缺少必需文件")?;
        ensure!(
            bytes.len() as u64 == record.size && digest(&bytes) == record.sha256,
            "资源内容校验失败"
        );
        members.insert(name.to_owned(), bytes);
    }
    let bytes = full
        .remove(&format!("{prefix}support/manifest.json"))
        .context("资源清单根目录无效")?;
    ensure!(
        full.is_empty() && members["WW-StudyApp.exe"] == members["Uninstall.exe"],
        "归档包含未知资源或卸载程序不一致"
    );
    manifest.files.push(OwnedFile {
        relative_path: "support/manifest.json".into(),
        role: FileRole::Manifest,
        size: bytes.len() as u64,
        sha256: digest(&bytes),
    });
    members.insert("support/manifest.json".into(), bytes);
    Ok(VerifiedArchive { manifest, members })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn package(
        root: &Path,
        version: &str,
        mutate: impl FnOnce(&mut PackageManifest, &mut BTreeMap<String, Vec<u8>>),
    ) -> (PathBuf, String) {
        let entries = [
            (
                "WW-StudyApp.exe",
                FileRole::Application,
                b"executable".as_slice(),
            ),
            (
                "Uninstall.exe",
                FileRole::Uninstaller,
                b"executable".as_slice(),
            ),
            (
                "vcruntime140.dll",
                FileRole::Runtime,
                b"runtime-a".as_slice(),
            ),
            (
                "vcruntime140_1.dll",
                FileRole::Runtime,
                b"runtime-b".as_slice(),
            ),
            ("support/LICENSE", FileRole::Notice, b"license".as_slice()),
            (
                "support/THIRD-PARTY-NOTICES.txt",
                FileRole::Notice,
                b"notices".as_slice(),
            ),
        ];
        let mut members = BTreeMap::new();
        let mut files = vec![];
        for (name, role, bytes) in entries {
            members.insert(name.into(), bytes.to_vec());
            files.push(OwnedFile {
                relative_path: name.into(),
                role,
                size: bytes.len() as u64,
                sha256: digest(bytes),
            });
        }
        let mut manifest = PackageManifest {
            schema: 1,
            product: PRODUCT.into(),
            version: version.into(),
            architecture: ARCHITECTURE.into(),
            files,
        };
        mutate(&mut manifest, &mut members);
        members.insert(
            "support/manifest.json".into(),
            serde_json::to_vec(&manifest).unwrap(),
        );
        let path = root.join(format!("{}.zip", uuid::Uuid::new_v4()));
        let file = File::create(&path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        for (name, bytes) in members {
            writer
                .start_file(
                    format!("WW-StudyApp-{version}-windows-x86_64-portable/{name}"),
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
            writer.write_all(&bytes).unwrap();
        }
        writer.finish().unwrap();
        let hash = digest(&std::fs::read(&path).unwrap());
        (path, hash)
    }
    fn root() -> PathBuf {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .join(".scratch/installation-20261026/test-temp")
            .join(format!("archive-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }
    #[test]
    fn maintenance_archive_validates_exact_resources_before_any_extraction() {
        let root = root();
        let (path, hash) = package(&root, "0.8.0", |_, _| {});
        let verified = read_archive(&path, &hash).unwrap();
        assert_eq!(verified.manifest.files.len(), 7);
        assert!(read_archive(&path, &"f".repeat(64)).is_err());
        for case in 0..5 {
            let (path, hash) = package(&root, "0.8.0", |manifest, members| match case {
                0 => manifest.schema = 2,
                1 => manifest.files[0].sha256 = "f".repeat(64),
                2 => {
                    members.insert("unknown.exe".into(), b"unknown".to_vec());
                }
                3 => {
                    members.insert("Uninstall.exe".into(), b"different".to_vec());
                }
                _ => {
                    members.insert("../outside".into(), b"outside".to_vec());
                }
            });
            assert!(read_archive(&path, &hash).is_err(), "case {case}");
        }
        assert!(!root.join("WW-StudyApp.exe").exists());
    }
    #[test]
    #[cfg(windows)]
    fn maintenance_reconciliation_only_creates_missing_verified_components() {
        let fixture = root();
        let (path, hash) = package(&fixture, env!("CARGO_PKG_VERSION"), |_, _| {});
        let root = fixture.join("portable");
        std::fs::create_dir(&root).unwrap();
        for (name, bytes) in [
            ("WW-StudyApp.exe", b"executable".as_slice()),
            ("vcruntime140.dll", b"runtime-a".as_slice()),
            ("vcruntime140_1.dll", b"runtime-b".as_slice()),
            ("README.md", b"old-user-readable-file".as_slice()),
        ] {
            std::fs::write(root.join(name), bytes).unwrap();
        }
        let receipt = super::super::reconcile_current_package(
            &path,
            &hash,
            &root.join("WW-StudyApp.exe"),
            &ProtectedPaths::default(),
        )
        .unwrap();
        assert_eq!(receipt.files.len(), 7);
        assert_eq!(
            std::fs::read(root.join("Uninstall.exe")).unwrap(),
            b"executable"
        );
        assert_eq!(
            std::fs::read(root.join("README.md")).unwrap(),
            b"old-user-readable-file"
        );
        let uninstaller = root.join("Uninstall.exe");
        let identity = runtime::observe_file(&uninstaller, "Uninstall.exe")
            .unwrap()
            .identity;
        super::super::file_ops::delete_exact(&uninstaller, &identity).unwrap();
        let repaired = super::super::reconcile_current_package(
            &path,
            &hash,
            &root.join("WW-StudyApp.exe"),
            &ProtectedPaths::default(),
        )
        .unwrap();
        assert_eq!(repaired.install_id, receipt.install_id);
        assert_eq!(std::fs::read(&uninstaller).unwrap(), b"executable");
        std::fs::write(root.join("Uninstall.exe"), b"unknown user file").unwrap();
        assert!(
            super::super::reconcile_current_package(
                &path,
                &hash,
                &root.join("WW-StudyApp.exe"),
                &ProtectedPaths::default()
            )
            .is_err()
        );
        assert_eq!(
            std::fs::read(root.join("Uninstall.exe")).unwrap(),
            b"unknown user file"
        );
    }
    #[test]
    fn maintenance_archive_update_version_must_equal_offered_release_before_staging() {
        let fixture = root();
        let (path, hash) = package(&fixture, "1.0.0", |_, _| {});
        let app = fixture.join("app");
        std::fs::create_dir_all(app.join("support")).unwrap();
        let bytes = b"runtime-a";
        std::fs::write(app.join("vcruntime140.dll"), bytes).unwrap();
        let identity = runtime::handle_identity(
            &crate::storage_backend::plain_handle(&app, true, false).unwrap(),
            true,
        )
        .unwrap()
        .0;
        let receipt = InstallReceipt {
            schema: 1,
            install_id: uuid::Uuid::new_v4().to_string(),
            owner: InstallOwner::Portable,
            install_root: app.clone(),
            root_identity: identity,
            version: "0.8.0".into(),
            source_package_sha256: "a".repeat(64),
            files: vec![OwnedFile {
                relative_path: "vcruntime140.dll".into(),
                role: FileRole::Runtime,
                size: bytes.len() as u64,
                sha256: digest(bytes),
            }],
        };
        assert!(
            super::super::prepare_portable_update(
                &path,
                &hash,
                &receipt,
                &ProtectedPaths::default(),
                "0.9.0"
            )
            .is_err()
        );
        assert!(!std::fs::read_dir(&app).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("WW-StudyApp-Recovery-")
        }));
    }
}
