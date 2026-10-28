use super::*;
use std::path::PathBuf;

fn file(name: &str) -> OwnedFile {
    OwnedFile {
        relative_path: name.into(),
        role: FileRole::Runtime,
        size: 4,
        sha256: "a".repeat(64),
    }
}
fn identity() -> FileIdentity {
    FileIdentity {
        volume: 1,
        file_id: 2,
        size: 4,
        sha256: "a".repeat(64),
    }
}
fn receipt() -> InstallReceipt {
    InstallReceipt {
        schema: 1,
        install_id: "fixture".into(),
        owner: InstallOwner::Portable,
        install_root: PathBuf::from(r"C:\Apps\Study"),
        root_identity: FileIdentity {
            size: 0,
            sha256: String::new(),
            ..identity()
        },
        version: "0.7.2".into(),
        source_package_sha256: "a".repeat(64),
        files: vec![file("app.dll"), file("old.dll")],
    }
}
fn manifest() -> PackageManifest {
    PackageManifest {
        schema: 1,
        product: PRODUCT.into(),
        version: "0.8.0".into(),
        architecture: ARCHITECTURE.into(),
        files: vec![file("app.dll"), file("new.dll")],
    }
}
fn observations(r: &InstallReceipt) -> Vec<ObservedFile> {
    r.files
        .iter()
        .map(|f| ObservedFile {
            relative_path: f.relative_path.clone(),
            identity: identity(),
            is_reparse: false,
            link_count: 1,
        })
        .collect()
}
fn preview(database: bool) -> UninstallPreview {
    preview_uninstall(
        &receipt(),
        DataSelection {
            configuration: false,
            database,
        },
        Some((PathBuf::from(r"C:/Library/studyapp.sqlite3"), identity())),
        vec![],
    )
    .unwrap()
}

#[test]
fn maintenance_rejects_windows_path_aliases() {
    for path in [
        "../x", "/x", "C:/x", "a:b", "a\\b", "", "a//b", "a/./b", "a/../b", "x.", "x ", "CON",
        "nul.txt", "COM1.dll", "a/aux", "a\0x",
    ] {
        assert!(normalize_relative_path(path).is_err(), "accepted {path:?}");
    }
    assert_eq!(
        normalize_relative_path("Support/Foo.dll").unwrap(),
        "support/foo.dll"
    );
}

#[test]
fn maintenance_manifest_refuses_unknown_metadata_and_data() {
    let good = manifest();
    assert!(validate_manifest(&good).is_ok());
    for bad in [
        PackageManifest {
            schema: 2,
            ..good.clone()
        },
        PackageManifest {
            product: "Other".into(),
            ..good.clone()
        },
        PackageManifest {
            architecture: "arm64".into(),
            ..good.clone()
        },
    ] {
        assert!(validate_manifest(&bad).is_err());
    }
    for name in [
        "settings/manifest.json",
        "studyapp.sqlite3",
        "notes/x.dll",
        "sync-config.json",
    ] {
        let bad = PackageManifest {
            files: vec![file(name)],
            ..good.clone()
        };
        assert!(validate_manifest(&bad).is_err(), "accepted {name}");
    }
    let mut bad = good.clone();
    bad.files.push(file("APP.DLL"));
    assert!(validate_manifest(&bad).is_err());
    let mut bad = good.clone();
    bad.files[0].sha256 = "z".repeat(64);
    assert!(validate_manifest(&bad).is_err());
    let mut bad = good;
    bad.files[0].size = 0;
    assert!(validate_manifest(&bad).is_err());
    assert!(serde_json::from_str::<FileRole>("\"database\"").is_err());
}

#[test]
fn maintenance_diff_owns_only_verified_old_files() {
    let r = receipt();
    let mut observed = observations(&r);
    observed.push(ObservedFile {
        relative_path: "neighbor.txt".into(),
        identity: identity(),
        is_reparse: false,
        link_count: 1,
    });
    let plan = plan_upgrade(&r, &manifest(), &observed, &ProtectedPaths::default()).unwrap();
    assert!(matches!(
        plan.operations.as_slice(),
        [
            FileOperation::Replace { .. },
            FileOperation::Add { .. },
            FileOperation::RemoveOwned { .. }
        ]
    ));
    observed[1].identity.sha256 = "b".repeat(64);
    let plan = plan_upgrade(&r, &manifest(), &observed, &ProtectedPaths::default()).unwrap();
    assert_eq!(plan.retained_conflicts, ["old.dll"]);
    assert_eq!(plan.operations.len(), 2);
}

#[test]
fn maintenance_collision_protected_root_and_unsafe_files_fail_closed() {
    let mut r = receipt();
    let mut observed = observations(&r);
    observed.push(ObservedFile {
        relative_path: "new.dll".into(),
        identity: identity(),
        is_reparse: false,
        link_count: 1,
    });
    assert!(plan_upgrade(&r, &manifest(), &observed, &ProtectedPaths::default()).is_err());
    let protected = ProtectedPaths {
        files: vec![],
        directories: vec![PathBuf::from(r"c:\APPS\study")],
    };
    assert!(plan_upgrade(&r, &manifest(), &observations(&r), &protected).is_err());
    let nearby = ProtectedPaths {
        files: vec![],
        directories: vec![PathBuf::from(r"C:\Apps\StudyOther")],
    };
    assert!(plan_upgrade(&r, &manifest(), &observations(&r), &nearby).is_ok());
    for (reparse, links) in [(true, 1), (false, 2)] {
        let mut o = observations(&r);
        o[0].is_reparse = reparse;
        o[0].link_count = links;
        assert!(plan_upgrade(&r, &manifest(), &o, &ProtectedPaths::default()).is_err());
    }
    r.owner = InstallOwner::LegacyPortable;
    assert!(
        plan_upgrade(
            &r,
            &manifest(),
            &observations(&r),
            &ProtectedPaths::default()
        )
        .is_err()
    );
}

#[test]
fn maintenance_database_requires_two_additional_confirmations() {
    let mut confirmation = UninstallConfirmation::new(preview(true));
    assert!(confirmation.confirm_database_review().is_err());
    confirmation.confirm_selection();
    assert_eq!(confirmation.stage(), ConfirmationStage::DatabaseReview);
    assert!(confirmation.clone().finish().is_err());
    confirmation.confirm_database_review().unwrap();
    assert!(confirmation.clone().finish().is_err());
    assert!(confirmation.confirm_database_final("yes").is_err());
    confirmation.confirm_database_final("删除数据库").unwrap();
    let token = confirmation.finish().unwrap();
    assert!(
        token
            .validate_database(Some((
                &PathBuf::from(r"C:/Library/studyapp.sqlite3"),
                &identity()
            )))
            .is_ok()
    );
    let changed = FileIdentity {
        file_id: 9,
        ..identity()
    };
    assert!(
        token
            .validate_database(Some((
                &PathBuf::from(r"C:/Library/studyapp.sqlite3"),
                &changed
            )))
            .is_err()
    );
    assert_eq!(
        UninstallConfirmation::new(token.into_preview()).stage(),
        ConfirmationStage::Selection
    );
}

#[test]
fn maintenance_default_selection_preserves_everything() {
    assert_eq!(
        DataSelection::default(),
        DataSelection {
            configuration: false,
            database: false
        }
    );
    let mut c = UninstallConfirmation::new(preview(false));
    c.confirm_selection();
    assert!(c.finish().is_ok());
    assert!(
        preview_uninstall(
            &receipt(),
            DataSelection {
                database: true,
                configuration: false
            },
            None,
            vec![]
        )
        .is_err()
    );
}

#[test]
fn maintenance_entry_parsing_is_exact_and_bounded() {
    assert_eq!(
        parse_entry_mode("WW-StudyApp.exe", &["--data-dir".into(), "fixture".into()]).unwrap(),
        EntryMode::Normal
    );
    assert_eq!(
        parse_entry_mode("UNINSTALL.EXE", &[]).unwrap(),
        EntryMode::UninstallInteractive
    );
    assert_eq!(
        parse_entry_mode("WW-StudyApp.exe", &["--uninstall".into()]).unwrap(),
        EntryMode::UninstallInteractive
    );
    assert_eq!(
        parse_entry_mode(
            "WW-StudyApp.exe",
            &["--maintenance-apply".into(), "a".repeat(32)]
        )
        .unwrap(),
        EntryMode::MaintenanceApply("a".repeat(32))
    );
    for args in [
        vec!["--delete-data=true".into()],
        vec!["--maintenance-apply".into(), "../command".into()],
        vec![
            "--uninstall".into(),
            "--maintenance-apply".into(),
            "a".repeat(32),
        ],
        vec!["--uninstall".into(), "--unknown".into()],
        vec!["--maintenance-apply".into()],
    ] {
        assert!(parse_entry_mode("WW-StudyApp.exe", &args).is_err());
    }
    assert!(parse_entry_mode("Uninstall.exe", &["--smoke-test".into()]).is_err());
}

#[test]
fn maintenance_explicit_isolated_data_context_is_read_only_input() {
    let args = vec![
        "--uninstall".into(),
        "--data-dir".into(),
        r"C:\isolated".into(),
    ];
    assert_eq!(
        parse_entry_mode("WW-StudyApp.exe", &args).unwrap(),
        EntryMode::UninstallInteractive
    );
    assert_eq!(
        maintenance_data_directory(&args).unwrap(),
        Some(PathBuf::from(r"C:\isolated"))
    );
    for args in [
        vec!["--uninstall".into(), "--data-dir".into()],
        vec!["--uninstall".into(), "--data-dir".into(), "../bad".into()],
        vec![
            "--uninstall".into(),
            "--data-dir".into(),
            r"C:\ok".into(),
            "--data-dir".into(),
            r"C:\other".into(),
        ],
    ] {
        assert!(parse_entry_mode("WW-StudyApp.exe", &args).is_err());
    }
}

#[test]
fn maintenance_msi_observation_has_no_invented_archive_digest() {
    let mut r = receipt();
    r.source_package_sha256.clear();
    assert!(validate_receipt(&r).is_err());
    r.owner = InstallOwner::Msi {
        product_code: "{FB2C8337-21E5-4406-83E2-4211C5691D1E}".into(),
        upgrade_code: MSI_UPGRADE_CODE.into(),
        user_sid: "S-1-5-21-1".into(),
    };
    assert!(validate_receipt(&r).is_ok());
    assert!(
        plan_upgrade(
            &r,
            &manifest(),
            &observations(&r),
            &ProtectedPaths::default()
        )
        .is_err()
    );
}

#[test]
fn maintenance_non_ascii_drive_prefix_is_rejected_without_panicking() {
    assert!(absolute_components(std::path::Path::new("C中文")).is_err());
}
