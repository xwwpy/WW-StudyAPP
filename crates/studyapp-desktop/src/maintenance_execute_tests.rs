use super::*;

#[test]
fn maintenance_result_ui_partial_and_rollback_text_does_not_claim_complete_cleanup() {
    let mut status = MaintenanceStatus {
        transaction_id: "fixture".into(),
        state: MaintenanceState::Failed,
        version: "0.8.0".into(),
        code: "application_removed_data_retained".into(),
        retained_paths: vec![],
    };
    assert!(
        crate::maintenance_ui::result_text_for_test(&status)
            .0
            .contains("未完成")
    );
    status.code = "partial_recovery".into();
    assert_eq!(
        crate::maintenance_ui::result_text_for_test(&status).0,
        "恢复尚未完成"
    );
    status.state = MaintenanceState::RolledBack;
    status.code = "interrupted_update_recovered".into();
    assert_eq!(
        crate::maintenance_ui::result_text_for_test(&status).0,
        "已恢复原有应用文件"
    );
}

pub(super) fn fixture() -> (PreparedMaintenance, PathBuf) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .join(".scratch/installation-20261026/test-temp")
        .join(format!("executor-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("support")).unwrap();
    std::fs::write(root.join("runtime.dll"), b"old-runtime").unwrap();
    std::fs::write(root.join("retired.dll"), b"old-retired").unwrap();
    std::fs::write(root.join("user-notes.md"), b"original-notes").unwrap();
    let owned = |name: &str| {
        let observed = runtime::observe_file(&root.join(name), name).unwrap();
        OwnedFile {
            relative_path: name.into(),
            role: FileRole::Runtime,
            size: observed.identity.size,
            sha256: observed.identity.sha256,
        }
    };
    let root_identity = runtime::handle_identity(
        &crate::storage_backend::plain_handle(&root, true, false).unwrap(),
        true,
    )
    .unwrap()
    .0;
    let receipt = InstallReceipt {
        schema: 1,
        install_id: uuid::Uuid::new_v4().to_string(),
        owner: InstallOwner::Portable,
        install_root: root.clone(),
        root_identity,
        version: "0.7.3".into(),
        source_package_sha256: "a".repeat(64),
        files: vec![owned("runtime.dll"), owned("retired.dll")],
    };
    std::fs::write(
        root.join(RECEIPT_PATH),
        serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();
    let (id, stage) = new_transaction(&root).unwrap();
    let mut operations = vec![];
    let mut next = receipt.clone();
    next.version = "0.8.0".into();
    next.files.clear();
    for (index, (name, bytes)) in [
        ("runtime.dll", Some(b"new-runtime".as_slice())),
        ("retired.dll", None),
        ("added.dll", Some(b"new-file".as_slice())),
    ]
    .into_iter()
    .enumerate()
    {
        let original = if root.join(name).exists() {
            Some(
                runtime::observe_file(&root.join(name), name)
                    .unwrap()
                    .identity,
            )
        } else {
            None
        };
        let staged = stage.join(format!("new-{index}"));
        let incoming = bytes.map(|bytes| create_new_file(&staged, bytes).unwrap());
        if let Some(identity) = &incoming {
            next.files.push(OwnedFile {
                relative_path: name.into(),
                role: FileRole::Runtime,
                size: identity.size,
                sha256: identity.sha256.clone(),
            });
        }
        operations.push(JournalOperation {
            relative_path: name.into(),
            original,
            incoming,
            staged,
            backup: stage.join(format!("old-{index}")),
        });
    }
    add_receipt_operation(&stage, &receipt, &next, &mut operations).unwrap();
    let prepared = descriptor(
        id,
        &stage,
        &JournalPlan {
            schema: 1,
            receipt,
            next_receipt: next,
            operations,
            retained_paths: vec![],
            protected_files: vec![],
            protected_directories: vec![],
            msi_package: None,
        },
        PayloadKind::PortableZip,
    )
    .unwrap();
    (prepared, root)
}
#[test]
fn maintenance_prepared_clones_and_twelve_cancel_cycles_release_owned_stages() {
    let (seed, root) = fixture();
    drop(seed);
    let receipt = runtime::read_portable_receipt(&root.join("WW-StudyApp.exe")).unwrap();
    for _ in 0..12 {
        let (id, stage) = new_transaction(&root).unwrap();
        let prepared = descriptor(
            id,
            &stage,
            &JournalPlan {
                schema: 1,
                receipt: receipt.clone(),
                next_receipt: receipt.clone(),
                operations: vec![],
                retained_paths: vec![],
                protected_files: vec![],
                protected_directories: vec![],
                msi_package: None,
            },
            PayloadKind::PortableZip,
        )
        .unwrap();
        let cloned = prepared.clone();
        drop(prepared);
        assert!(stage.exists());
        drop(cloned);
        assert!(!stage.exists());
    }
    for _ in 0..12 {
        let (_, stage) = new_transaction(&root).unwrap();
        let guard = crate::maintenance_handoff::cleanup_stage_on_error(&stage).unwrap();
        create_new_file(&stage.join("new-0"), b"partial preparation").unwrap();
        drop(guard);
        assert!(!stage.exists());
    }
    let (_, stage) = new_transaction(&root).unwrap();
    let guard = crate::maintenance_handoff::cleanup_stage_on_error(&stage).unwrap();
    create_new_file(&stage.join("new-0"), b"owned").unwrap();
    std::fs::write(stage.join("new-0"), b"unknown replacement").unwrap();
    drop(guard);
    assert_eq!(
        std::fs::read(stage.join("new-0")).unwrap(),
        b"unknown replacement"
    );
    assert!(new_transaction(&root).is_ok());
}
#[test]
fn maintenance_lock_path_cannot_be_replaced_or_acquired_twice() {
    let (_, root) = fixture();
    let lock = installation_lock(&root).unwrap();
    assert!(
        std::fs::rename(
            root.join("support/maintenance.lock"),
            root.join("moved-lock")
        )
        .is_err()
    );
    assert!(installation_lock(&root).is_err());
    drop(lock);
    std::fs::write(root.join("studyapp.lock"), b"").unwrap();
    let lock = runtime::acquire_existing_library_lock(&root).unwrap();
    assert!(std::fs::rename(root.join("studyapp.lock"), root.join("moved-library-lock")).is_err());
    assert!(runtime::acquire_existing_library_lock(&root).is_err());
    drop(lock);
}
#[test]
fn maintenance_transaction_commits_receipt_after_payload_and_preserves_neighbors() {
    let (prepared, root) = fixture();
    let status = apply_with_health(
        &prepared,
        |plan| {
            assert_eq!(std::fs::read(root.join("runtime.dll"))?, b"new-runtime");
            assert_eq!(
                runtime::read_portable_receipt(&root.join("WW-StudyApp.exe"))?.version,
                plan.next_receipt.version
            );
            Ok(())
        },
        |_, _| Ok(()),
    )
    .unwrap();
    assert_eq!(status.state, MaintenanceState::Succeeded);
    assert_eq!(
        std::fs::read(root.join("user-notes.md")).unwrap(),
        b"original-notes"
    );
    assert!(!root.join("retired.dll").exists());
    let plan = load_plan(&prepared).unwrap();
    assert!(
        plan.operations
            .iter()
            .all(|op| !op.backup.exists() && !op.staged.exists())
    );
}
#[test]
fn maintenance_transaction_rolls_back_each_mutation_boundary_and_health_failure() {
    for index in 0..4 {
        for boundary in ["before", "backed_up", "installed"] {
            let (prepared, root) = fixture();
            let status = apply_with_health(
                &prepared,
                |_| Ok(()),
                |event, current| {
                    if event == boundary && current == index {
                        bail!("injected disk or permission failure");
                    }
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(
                status.state,
                MaintenanceState::RolledBack,
                "{boundary} {index}"
            );
            assert_eq!(
                std::fs::read(root.join("runtime.dll")).unwrap(),
                b"old-runtime"
            );
            assert_eq!(
                std::fs::read(root.join("retired.dll")).unwrap(),
                b"old-retired"
            );
            assert!(!root.join("added.dll").exists());
            assert_eq!(
                runtime::read_portable_receipt(&root.join("WW-StudyApp.exe"))
                    .unwrap()
                    .version,
                "0.7.3"
            );
        }
    }
    let (prepared, root) = fixture();
    let status = apply_with_health(&prepared, |_| bail!("health failed"), |_, _| Ok(())).unwrap();
    assert_eq!(status.state, MaintenanceState::RolledBack);
    assert_eq!(
        std::fs::read(root.join("runtime.dll")).unwrap(),
        b"old-runtime"
    );
}
#[test]
fn maintenance_transaction_preserves_external_replacement_during_rollback() {
    let (prepared, root) = fixture();
    let status = apply_with_health(
        &prepared,
        |_| Ok(()),
        |event, index| {
            if event == "installed" && index == 0 {
                std::fs::rename(root.join("runtime.dll"), root.join("externally-moved.dll"))?;
                std::fs::write(root.join("runtime.dll"), b"external-content")?;
                bail!("changed during apply");
            }
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(status.state, MaintenanceState::Failed);
    assert_eq!(status.code, "partial_recovery");
    assert_eq!(
        std::fs::read(root.join("runtime.dll")).unwrap(),
        b"external-content"
    );
    assert!(prepared.plan_path.parent().unwrap().join("old-0").exists());
}
#[test]
fn maintenance_transaction_rejects_tampered_plan_and_wrong_owner_before_mutation() {
    let (mut prepared, root) = fixture();
    prepared.kind = PayloadKind::Msi;
    assert!(apply_with_health(&prepared, |_| Ok(()), |_, _| Ok(())).is_err());
    prepared.kind = PayloadKind::PortableZip;
    std::fs::write(&prepared.plan_path, b"tampered").unwrap();
    assert!(apply_with_health(&prepared, |_| Ok(()), |_, _| Ok(())).is_err());
    assert_eq!(
        std::fs::read(root.join("runtime.dll")).unwrap(),
        b"old-runtime"
    );
}
#[test]
fn maintenance_transaction_locked_and_multilink_targets_never_force_mutation() {
    let (prepared, root) = fixture();
    let held =
        crate::storage_backend::plain_handle(&root.join("runtime.dll"), false, true).unwrap();
    let status = apply_with_health(&prepared, |_| Ok(()), |_, _| Ok(())).unwrap();
    assert_eq!(status.state, MaintenanceState::RolledBack);
    assert_eq!(
        std::fs::read(root.join("runtime.dll")).unwrap(),
        b"old-runtime"
    );
    drop(held);
    let (prepared, root) = fixture();
    std::fs::hard_link(root.join("runtime.dll"), root.join("user-alias.dll")).unwrap();
    assert!(apply_with_health(&prepared, |_| Ok(()), |_, _| Ok(())).is_err());
    assert_eq!(
        std::fs::read(root.join("user-alias.dll")).unwrap(),
        b"old-runtime"
    );
}
#[test]
fn maintenance_transaction_held_directory_cannot_be_swapped_mid_apply() {
    let (prepared, root) = fixture();
    let status = apply_with_health(
        &prepared,
        |_| Ok(()),
        |event, index| {
            if event == "before" && index == 0 {
                assert!(std::fs::rename(root.join("support"), root.join("moved-support")).is_err());
            }
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(status.state, MaintenanceState::Succeeded);
    assert!(!root.join("moved-support").exists());
}
#[test]
fn maintenance_completed_journal_cleanup_is_bounded_and_keeps_unknown_files() {
    let (prepared, root) = fixture();
    let status = apply_with_health(&prepared, |_| Ok(()), |_, _| Ok(())).unwrap();
    assert_eq!(status.state, MaintenanceState::Succeeded);
    let current = runtime::read_portable_receipt(&root.join("WW-StudyApp.exe")).unwrap();
    let protected_stage = prepared.plan_path.parent().unwrap().to_owned();
    std::fs::write(protected_stage.join("unknown.txt"), b"keep unknown").unwrap();
    for _ in 0..10 {
        let (id, stage) = new_transaction(&root).unwrap();
        let plan = JournalPlan {
            schema: 1,
            receipt: current.clone(),
            next_receipt: current.clone(),
            operations: vec![],
            retained_paths: vec![],
            protected_files: vec![],
            protected_directories: vec![],
            msi_package: None,
        };
        let prepared = descriptor(id, &stage, &plan, PayloadKind::PortableZip).unwrap();
        recovery::mark_completed(&prepared, MaintenanceState::Succeeded).unwrap();
        write_status(
            &prepared,
            &MaintenanceStatus {
                transaction_id: prepared.transaction_id.clone(),
                state: MaintenanceState::Succeeded,
                version: current.version.clone(),
                code: "fixture_completed".into(),
                retained_paths: vec![],
            },
        )
        .unwrap();
    }
    assert_eq!(
        std::fs::read(protected_stage.join("unknown.txt")).unwrap(),
        b"keep unknown"
    );
    assert!(protected_stage.join("plan.json").exists());
    let count = std::fs::read_dir(&root)
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("WW-StudyApp-Recovery-")
        })
        .count();
    assert!(count <= 3, "retained {count}");
}
