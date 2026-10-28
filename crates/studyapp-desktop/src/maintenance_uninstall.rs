//! Confirmed, individually captured data cleanup. Never scans tracked source paths.
use super::*;

pub(crate) fn prepare_uninstall(
    confirmed: ConfirmedUninstall,
    receipt: &InstallReceipt,
    context: &runtime::DiscoveryContext,
) -> Result<AuthenticatedUninstall> {
    let preview = confirmed.into_preview();
    validate_receipt(receipt)?;
    ensure!(
        preview.install_id == receipt.install_id && preview.application_files == receipt.files,
        "卸载预览与当前安装不符"
    );
    let _guards = checked_root(receipt)?;
    let library = runtime::discover_library(context);
    let location_file = if context.explicit_library.is_none() {
        context.location_file.clone()
    } else {
        None
    };
    let library_path = library
        .database_path
        .as_ref()
        .and_then(|p| p.parent())
        .map(Path::to_owned);
    let same_manifest = PackageManifest {
        schema: 1,
        product: PRODUCT.into(),
        architecture: ARCHITECTURE.into(),
        version: receipt.version.clone(),
        files: receipt.files.clone(),
    };
    if receipt.owner == InstallOwner::Portable {
        let protected = ProtectedPaths {
            files: library.configuration_files.clone(),
            directories: library_path.iter().cloned().collect(),
        };
        plan_upgrade(
            receipt,
            &same_manifest,
            &observe_union(receipt, &same_manifest)?,
            &protected,
        )?;
    }
    if preview.selection.database || preview.selection.configuration {
        ensure!(library.issues.is_empty(), "资料库或配置检查未通过");
        ensure!(
            library.verified_database
                == preview
                    .database_path
                    .clone()
                    .zip(preview.database_identity.clone()),
            "资料库身份已改变"
        );
    }
    let mut configuration_files = vec![];
    let mut database_files = vec![];
    if preview.selection.configuration {
        ensure!(
            preview.configuration_files == library.configuration_files,
            "配置清单已改变"
        );
        for path in preview.configuration_files {
            configuration_files.push(CapturedFile {
                identity: runtime::observe_file(&path, "configuration")?.identity,
                path,
            });
        }
    }
    if preview.selection.database {
        let database = preview.database_path.as_ref().context("缺少已确认数据库")?;
        for path in [
            database.clone(),
            database.with_file_name("studyapp.sqlite3-wal"),
            database.with_file_name("studyapp.sqlite3-shm"),
        ] {
            if path.try_exists()? {
                database_files.push(CapturedFile {
                    identity: runtime::observe_file(&path, "database")?.identity,
                    path,
                });
            }
        }
    }
    Ok(AuthenticatedUninstall {
        receipt: receipt.clone(),
        transaction_id: uuid::Uuid::new_v4().simple().to_string(),
        configuration: preview.selection.configuration,
        database: preview.selection.database,
        database_path: preview.database_path,
        database_identity: preview.database_identity,
        configuration_files,
        database_files,
        library_path,
        location_file,
    })
}

pub(crate) fn apply_authenticated_uninstall(
    intent: AuthenticatedUninstall,
) -> Result<MaintenanceStatus> {
    validate_receipt(&intent.receipt)?;
    ensure!(
        (intent.configuration || intent.configuration_files.is_empty())
            && (intent.database || intent.database_files.is_empty()),
        "卸载清理范围与确认不符"
    );
    ensure!(
        intent.configuration_files.len() <= 2048 && intent.database_files.len() <= 3,
        "卸载清理资源数量无效"
    );
    ensure!(
        intent.transaction_id.len() == 32
            && intent.transaction_id.bytes().all(|c| c.is_ascii_hexdigit()),
        "卸载事务引用无效"
    );
    let mut status = MaintenanceStatus {
        transaction_id: intent.transaction_id.clone(),
        state: MaintenanceState::Failed,
        version: intent.receipt.version.clone(),
        code: "application_removal_failed".into(),
        retained_paths: vec![],
    };
    let app_result = match &intent.receipt.owner {
        InstallOwner::Msi { product_code, .. } => {
            msi::verify_registration(&intent.receipt)?;
            let controls = capture_controls(&intent.receipt, None)?;
            let code = msi::invoke("/x", std::ffi::OsStr::new(product_code))?;
            if msi_exit_code(code).is_err() {
                status.code = format!("msi_{code}");
                return Ok(status);
            }
            let registrations = runtime::registered_current_user_msi()?;
            ensure!(
                runtime::matching_msi_owner(
                    &intent.receipt.install_root.join("WW-StudyApp.exe"),
                    &registrations
                )?
                .is_none(),
                "MSI 所有者仍然存在"
            );
            if code == 3010 {
                status.code = "reboot_required_data_retained".into();
                return Ok(status);
            }
            Ok(controls)
        }
        InstallOwner::Portable => remove_portable(&intent.receipt, &mut status.retained_paths),
        InstallOwner::LegacyPortable => bail!("旧便携版尚未建立所有权"),
    };
    let controls = if let Ok(controls) = app_result {
        controls
    } else {
        #[cfg(test)]
        if let Err(error) = &app_result {
            eprintln!("application removal: {error:#}");
        }
        for resource in &intent.receipt.files {
            let path = intent.receipt.install_root.join(&resource.relative_path);
            if path.try_exists().unwrap_or(true) && !status.retained_paths.contains(&path) {
                status.retained_paths.push(path);
            }
        }
        return Ok(status);
    };
    status.code = "complete".into();
    status.state = MaintenanceState::Succeeded;
    if (intent.configuration || intent.database)
        && cleanup_data(&intent, &mut status.retained_paths).is_err()
    {
        status.state = MaintenanceState::Failed;
        status.code = "application_removed_data_retained".into();
        for file in intent
            .configuration_files
            .iter()
            .chain(&intent.database_files)
        {
            if file.path.try_exists().unwrap_or(true) {
                status.retained_paths.push(file.path.clone());
            }
        }
        if let Some(path) = &intent.database_path
            && path.try_exists().unwrap_or(true)
            && !status.retained_paths.contains(path)
        {
            status.retained_paths.push(path.clone());
        }
    }
    cleanup_controls(controls, &mut status.retained_paths);
    if status.state == MaintenanceState::Succeeded && !status.retained_paths.is_empty() {
        status.code = "owned_cleanup_retained".into();
    }
    Ok(status)
}
fn remove_portable(
    receipt: &InstallReceipt,
    retained: &mut Vec<PathBuf>,
) -> Result<ControlCleanup> {
    let _guards = checked_root(receipt)?;
    let _lock = installation_lock(&receipt.install_root)?;
    let controls = capture_controls(receipt, Some(&_lock))?;
    ensure!(
        runtime::current_payload_kind(&receipt.install_root.join("WW-StudyApp.exe"))?
            == PayloadKind::PortableZip,
        "安装所有者已改变"
    );
    let current = runtime::read_portable_receipt(&receipt.install_root.join("WW-StudyApp.exe"))?;
    ensure!(current == *receipt, "安装记录已改变");
    let mut captured = vec![];
    for resource in &receipt.files {
        let path = receipt.install_root.join(&resource.relative_path);
        let observed = runtime::observe_file(&path, &resource.relative_path)?;
        ensure!(
            observed.link_count == 1
                && observed.identity.size == resource.size
                && observed.identity.sha256 == resource.sha256,
            "应用资源已改变，停止卸载"
        );
        captured.push(CapturedFile {
            path,
            identity: observed.identity,
        });
    }
    let receipt_path = receipt.install_root.join(RECEIPT_PATH);
    captured.push(CapturedFile {
        identity: runtime::observe_file(&receipt_path, RECEIPT_PATH)?.identity,
        path: receipt_path,
    });
    for captured in captured {
        if let Err(error) = file_ops::delete_exact(&captured.path, &captured.identity) {
            retained.push(captured.path);
            return Err(error);
        }
    }
    Ok(controls)
}
struct ControlCleanup {
    files: Vec<CapturedFile>,
    directories: Vec<CapturedFile>,
}
fn verify_control_owner(
    plan: &JournalPlan,
    receipt: &InstallReceipt,
    state: &MaintenanceState,
) -> Result<()> {
    ensure!(
        plan.receipt.root_identity == receipt.root_identity
            && same_absolute_path(&plan.receipt.install_root, &receipt.install_root)?,
        "维护控制文件所有者不符"
    );
    if plan.receipt.install_id == receipt.install_id {
        return Ok(());
    }
    ensure!(
        *state == MaintenanceState::Succeeded && plan.msi_package.is_some(),
        "维护控制文件所有者不符"
    );
    msi::verify_transition(&plan.receipt, &plan.next_receipt)?;
    msi::verify_target_receipt(&plan.next_receipt, receipt)
}
fn capture_controls(
    receipt: &InstallReceipt,
    lock_handle: Option<&File>,
) -> Result<ControlCleanup> {
    let _guards = checked_root(receipt)?;
    let mut controls = ControlCleanup {
        files: vec![],
        directories: vec![],
    };
    let root = &receipt.install_root;
    let capture_directory = |path: PathBuf| -> Result<CapturedFile> {
        let file = crate::storage_backend::plain_handle(&path, true, false)?;
        Ok(CapturedFile {
            identity: runtime::handle_identity(&file, true)?.0,
            path,
        })
    };
    for entry in std::fs::read_dir(root)?.take(1024) {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(id) = name.strip_prefix("WW-StudyApp-Recovery-") else {
            continue;
        };
        if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let stage = entry.path();
        let inspected = || -> Result<Vec<CapturedFile>> {
            let _stage = crate::storage_backend::guard_directories(&stage, false)?;
            let (status, _) =
                read_captured_json::<MaintenanceStatus>(&stage.join("recovered.json"))
                    .or_else(|_| read_captured_json(&stage.join("status.json")))?;
            ensure!(
                status.transaction_id == id
                    && matches!(
                        status.state,
                        MaintenanceState::Succeeded | MaintenanceState::RolledBack
                    ),
                "维护事务未完成"
            );
            let (plan, plan_identity) =
                read_captured_json::<JournalPlan>(&stage.join("plan.json"))?;
            recovery::validate_control_plan(&stage, &plan_identity)?;
            ensure!(
                recovery::completed_state(&stage, id, &plan_identity)?
                    == Some(status.state.clone()),
                "维护完成记录未通过验证"
            );
            verify_control_owner(&plan, receipt, &status.state)?;
            let prepared = PreparedMaintenance {
                _lease: None,
                transaction_id: id.into(),
                plan_path: stage.join("plan.json"),
                plan_sha256: plan_identity.sha256,
                install_root: root.clone(),
                status_path: stage.join("status.json"),
                kind: if plan.msi_package.is_some() {
                    PayloadKind::Msi
                } else {
                    PayloadKind::PortableZip
                },
                restart_args: vec![],
                receipt: plan.receipt.clone(),
            };
            load_plan(&prepared)?;
            let mut files = vec![];
            for operation in &plan.operations {
                for (path, expected) in [
                    (&operation.staged, &operation.incoming),
                    (&operation.backup, &operation.original),
                ] {
                    if let Some(expected) = expected
                        && path.try_exists()?
                    {
                        let observed = runtime::observe_file(path, "transaction")?;
                        ensure!(
                            observed.link_count == 1 && observed.identity == *expected,
                            "维护资源已改变"
                        );
                        files.push(CapturedFile {
                            path: path.clone(),
                            identity: observed.identity,
                        });
                    }
                }
            }
            for name in [
                "plan.json",
                "status.json",
                "journal.jsonl",
                "recovery.bin",
                "completed.bin",
                "recovered.json",
                "recovery-partial.json",
                "cleanup.json",
            ] {
                let path = stage.join(name);
                if path.try_exists()? {
                    let observed = runtime::observe_file(&path, "control")?;
                    ensure!(observed.link_count == 1, "维护控制文件存在多个链接");
                    files.push(CapturedFile {
                        path,
                        identity: observed.identity,
                    });
                }
            }
            for (name, expected) in crate::maintenance_handoff::recorded_files(&stage)? {
                let path = stage.join(name);
                if path.try_exists()? && !files.iter().any(|file| file.path == path) {
                    let observed = runtime::observe_file(&path, "snapshot")?;
                    ensure!(
                        observed.link_count == 1 && observed.identity == expected,
                        "恢复快照资源已改变"
                    );
                    files.push(CapturedFile {
                        path,
                        identity: expected,
                    });
                }
            }
            Ok(files)
        };
        if let Ok(files) = inspected() {
            controls.files.extend(files);
            controls.directories.push(capture_directory(stage)?);
        }
    }
    let lock = root.join("support/maintenance.lock");
    if lock.try_exists()? {
        let (identity, links) = if let Some(handle) = lock_handle {
            runtime::handle_identity(handle, false)?
        } else {
            let observed = runtime::observe_file(&lock, "lock")?;
            (observed.identity, observed.link_count)
        };
        ensure!(links == 1 && identity.size == 0, "维护锁不是已知空控制文件");
        controls.files.push(CapturedFile {
            path: lock,
            identity,
        });
    }
    let support = root.join("support");
    if support.try_exists()? {
        controls.directories.push(capture_directory(support)?);
    }
    controls.directories.push(capture_directory(root.clone())?);
    Ok(controls)
}
fn cleanup_controls(controls: ControlCleanup, retained: &mut Vec<PathBuf>) {
    for captured in controls.files {
        match captured.path.try_exists() {
            Ok(false) => {}
            Ok(true) => {
                if file_ops::delete_exact(&captured.path, &captured.identity).is_err() {
                    retained.push(captured.path);
                }
            }
            Err(_) => retained.push(captured.path),
        }
    }
    for captured in controls.directories {
        match captured.path.try_exists() {
            Ok(false) => {}
            Ok(true) => {
                if file_ops::delete_empty_directory(&captured.path, &captured.identity).is_err() {
                    retained.push(captured.path);
                }
            }
            Err(_) => retained.push(captured.path),
        }
    }
}
fn cleanup_data(intent: &AuthenticatedUninstall, retained: &mut Vec<PathBuf>) -> Result<()> {
    let library = intent.library_path.as_ref().context("资料库位置无法验证")?;
    let _guards = crate::storage_backend::guard_directories(library, false)?;
    let _lock = runtime::acquire_existing_library_lock(library)?;
    let expected_database = library.join("studyapp.sqlite3");
    if let Some(database) = &intent.database_path {
        ensure!(
            same_absolute_path(database, &expected_database)?,
            "资料库位置不符"
        );
    }
    if let Some(identity) = &intent.database_identity {
        ensure!(
            runtime::observe_file(&expected_database, "database")?.identity == *identity,
            "数据库身份已改变"
        );
    }
    if intent.configuration {
        let inventory = runtime::configuration_inventory(library, intent.location_file.as_deref())?;
        ensure!(
            intent
                .configuration_files
                .iter()
                .map(|f| &f.path)
                .eq(inventory.iter()),
            "配置清单已改变"
        );
    }
    for file in &intent.configuration_files {
        ensure!(
            runtime::observe_file(&file.path, "configuration")?.identity == file.identity,
            "配置文件已改变"
        );
    }
    if intent.database {
        ensure!(intent.database_identity.is_some(), "缺少已确认数据库身份");
        let actual = [
            expected_database.clone(),
            library.join("studyapp.sqlite3-wal"),
            library.join("studyapp.sqlite3-shm"),
        ];
        for path in &actual {
            let captured = intent.database_files.iter().find(|f| f.path == *path);
            match (path.try_exists()?, captured) {
                (true, Some(file)) => ensure!(
                    runtime::observe_file(path, "database")?.identity == file.identity,
                    "数据库附属文件已改变"
                ),
                (false, None) => {}
                _ => bail!("数据库文件集合已改变"),
            }
        }
        ensure!(
            intent.database_files.len() <= 3
                && intent
                    .database_files
                    .iter()
                    .all(|f| actual.contains(&f.path)),
            "数据库清理路径无效"
        );
    } else if intent.configuration && expected_database.try_exists()? {
        let file = runtime::no_delete_file_handle(&expected_database)?;
        let (identity, links) = runtime::handle_identity(&file, false)?;
        ensure!(
            links == 1 && Some(&identity) == intent.database_identity.as_ref(),
            "数据库身份已改变"
        );
        // Pin each possible SQLite write sidecar before SQLite opens its path.
        // CreateNew on missing sidecars closes the otherwise unguardable alias race.
        let sidecars = guard_sqlite_sidecars(&expected_database)?;
        studyapp_core::Engine::reset_existing_preferences_for_uninstall(&expected_database)?;
        drop(sidecars);
    }
    let location = intent.location_file.as_ref();
    for file in intent
        .configuration_files
        .iter()
        .filter(|f| Some(&f.path) != location)
        .chain(&intent.database_files)
        .chain(
            intent
                .configuration_files
                .iter()
                .filter(|f| Some(&f.path) == location),
        )
    {
        if let Err(error) = file_ops::delete_exact(&file.path, &file.identity) {
            retained.push(file.path.clone());
            return Err(error);
        }
    }
    Ok(())
}

fn guard_sqlite_sidecars(database: &Path) -> Result<Vec<File>> {
    let parent = database.parent().context("资料库目录无效")?;
    let _guards = crate::storage_backend::guard_directories(parent, false)?;
    let mut files = vec![];
    for name in [
        "studyapp.sqlite3-wal",
        "studyapp.sqlite3-shm",
        "studyapp.sqlite3-journal",
    ] {
        let path = parent.join(name);
        let file = if path.try_exists()? {
            runtime::no_delete_file_handle(&path)?
        } else {
            let mut options = OpenOptions::new();
            options.read(true).write(true).create_new(true);
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                options.share_mode(3).custom_flags(0x0020_0000);
            }
            // If another creator won, fail closed rather than opening its replacement.
            let file = options.open(&path)?;
            file.sync_all()?;
            ensure!(
                runtime::handle_identity(&file, false)?.1 == 1,
                "数据库附属文件存在多个链接"
            );
            file
        };
        files.push(file);
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn msi_owner_transition_fixture() -> (PreparedMaintenance, InstallReceipt) {
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .join(".scratch/installation-20261026/test-temp")
            .join(format!("msi-owner-transition-{}", uuid::Uuid::new_v4()));
        let root = base.join("application");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir(base.join("external")).unwrap();
        let package_path = base.join("external/package.msi");
        let package_identity =
            create_new_file(&package_path, b"synthetic package identity").unwrap();
        let receipt = InstallReceipt {
            schema: 1,
            install_id: "11111111-1111-1111-1111-111111111111".into(),
            owner: InstallOwner::Msi {
                product_code: "{11111111-1111-1111-1111-111111111111}".into(),
                upgrade_code: MSI_UPGRADE_CODE.into(),
                user_sid: "S-1-5-21-1".into(),
            },
            install_root: root.clone(),
            root_identity: runtime::handle_identity(
                &crate::storage_backend::plain_handle(&root, true, false).unwrap(),
                true,
            )
            .unwrap()
            .0,
            version: "0.7.3".into(),
            source_package_sha256: String::new(),
            files: vec![OwnedFile {
                relative_path: "runtime.dll".into(),
                role: FileRole::Runtime,
                size: 3,
                sha256: archive::digest(b"old"),
            }],
        };
        let mut next = receipt.clone();
        next.install_id = "22222222-2222-2222-2222-222222222222".into();
        if let InstallOwner::Msi { product_code, .. } = &mut next.owner {
            *product_code = format!("{{{}}}", next.install_id);
        }
        next.version = "0.8.0".into();
        next.source_package_sha256 = package_identity.sha256.clone();
        let (id, stage) = new_transaction(&root).unwrap();
        let prepared = descriptor(
            id,
            &stage,
            &JournalPlan {
                schema: 1,
                receipt,
                next_receipt: next.clone(),
                operations: vec![],
                retained_paths: vec![],
                protected_files: vec![],
                protected_directories: vec![],
                msi_package: Some(CapturedFile {
                    path: package_path,
                    identity: package_identity,
                }),
            },
            PayloadKind::Msi,
        )
        .unwrap();
        recovery::mark_completed(&prepared, MaintenanceState::Succeeded).unwrap();
        write_status(
            &prepared,
            &MaintenanceStatus {
                transaction_id: prepared.transaction_id.clone(),
                state: MaintenanceState::Succeeded,
                version: next.version.clone(),
                code: "complete".into(),
                retained_paths: vec![],
            },
        )
        .unwrap();
        next.source_package_sha256.clear();
        (prepared, next)
    }
    #[test]
    fn msi_owner_transition_captures_completed_controls_and_preserves_unknown_files() {
        let (prepared, current) = msi_owner_transition_fixture();
        let stage = prepared.plan_path.parent().unwrap();
        let unknown = stage.join("unknown.txt");
        std::fs::write(&unknown, b"user-owned neighbor").unwrap();
        let controls = capture_controls(&current, None).unwrap();
        assert!(
            controls
                .files
                .iter()
                .any(|file| file.path == prepared.plan_path),
            "authenticated completed MSI transition must capture its old-owner journal"
        );
        let mut retained = vec![];
        cleanup_controls(controls, &mut retained);
        assert!(!prepared.plan_path.exists());
        assert!(!stage.join("completed.bin").exists());
        assert_eq!(std::fs::read(&unknown).unwrap(), b"user-owned neighbor");
        assert!(retained.contains(&stage.to_owned()));
    }
    #[test]
    fn msi_owner_transition_rejects_tampered_completion_and_changed_current_owner() {
        let (prepared, current) = msi_owner_transition_fixture();
        let mut changed = current.clone();
        changed.install_id = "33333333-3333-3333-3333-333333333333".into();
        if let InstallOwner::Msi { product_code, .. } = &mut changed.owner {
            *product_code = format!("{{{}}}", changed.install_id);
        }
        assert!(
            capture_controls(&changed, None)
                .unwrap()
                .files
                .iter()
                .all(|file| file.path != prepared.plan_path)
        );
        let completion = prepared.plan_path.parent().unwrap().join("completed.bin");
        std::fs::write(&completion, b"untrusted completion").unwrap();
        let controls = capture_controls(&current, None).unwrap();
        assert!(
            controls
                .files
                .iter()
                .all(|file| file.path != prepared.plan_path)
        );
        assert_eq!(std::fs::read(&completion).unwrap(), b"untrusted completion");
    }
    #[test]
    fn msi_owner_transition_binds_validated_package_target_before_execution() {
        let (prepared, _) = msi_owner_transition_fixture();
        let plan = load_plan(&prepared).unwrap();
        let target_product = "{22222222-2222-2222-2222-222222222222}";
        let hash = &plan.msi_package.as_ref().unwrap().identity.sha256;
        let target = msi::planned_receipt(&plan.receipt, target_product, "0.8.0", hash).unwrap();
        assert_eq!(target, plan.next_receipt);
        assert!(msi::verify_package_target(&plan.receipt, &target, target_product, hash).is_ok());
        for (product, package_hash) in [
            ("{33333333-3333-3333-3333-333333333333}", hash.as_str()),
            (
                target_product,
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
        ] {
            assert!(
                msi::verify_package_target(&plan.receipt, &target, product, package_hash).is_err()
            );
        }
        let mut legacy_target = target.clone();
        legacy_target.owner = plan.receipt.owner.clone();
        legacy_target.install_id = plan.receipt.install_id.clone();
        assert!(
            msi::verify_package_target(&plan.receipt, &legacy_target, target_product, hash)
                .is_err()
        );
        for (product, version, package_hash) in [
            (
                "{11111111-1111-1111-1111-111111111111}",
                "0.8.0",
                hash.as_str(),
            ),
            ("not-a-product-code", "0.8.0", hash.as_str()),
            (target_product, "0.7.3", hash.as_str()),
            (target_product, "0.7.2", hash.as_str()),
            (target_product, "0.8.0", ""),
        ] {
            assert!(msi::planned_receipt(&plan.receipt, product, version, package_hash).is_err());
        }
    }
    #[test]
    fn msi_owner_transition_rejects_unbound_owners_roots_versions_and_states() {
        let (prepared, current) = msi_owner_transition_fixture();
        let plan = load_plan(&prepared).unwrap();
        assert!(verify_control_owner(&plan, &current, &MaintenanceState::Succeeded).is_ok());
        type Change = fn(&mut JournalPlan, &mut InstallReceipt);
        let cases: &[(&str, Change)] = &[
            ("old id", |plan, _| {
                plan.receipt.install_id = "unbound".into()
            }),
            ("next id", |plan, _| {
                plan.next_receipt.install_id = "unbound".into()
            }),
            ("current id", |_, current| {
                current.install_id = "unbound".into()
            }),
            ("old portable", |plan, _| {
                plan.receipt.owner = InstallOwner::Portable
            }),
            ("next portable", |plan, _| {
                plan.next_receipt.owner = InstallOwner::Portable
            }),
            ("current portable", |_, current| {
                current.owner = InstallOwner::Portable
            }),
            ("old sid", |plan, _| {
                if let InstallOwner::Msi { user_sid, .. } = &mut plan.receipt.owner {
                    *user_sid = "S-1-5-21-2".into();
                }
            }),
            ("next sid", |plan, _| {
                if let InstallOwner::Msi { user_sid, .. } = &mut plan.next_receipt.owner {
                    *user_sid = "S-1-5-21-2".into();
                }
            }),
            ("current sid", |_, current| {
                if let InstallOwner::Msi { user_sid, .. } = &mut current.owner {
                    *user_sid = "S-1-5-21-2".into();
                }
            }),
            ("old family", |plan, _| {
                if let InstallOwner::Msi { upgrade_code, .. } = &mut plan.receipt.owner {
                    *upgrade_code = "{33333333-3333-3333-3333-333333333333}".into();
                }
            }),
            ("next family", |plan, _| {
                if let InstallOwner::Msi { upgrade_code, .. } = &mut plan.next_receipt.owner {
                    *upgrade_code = "{33333333-3333-3333-3333-333333333333}".into();
                }
            }),
            ("current family", |_, current| {
                if let InstallOwner::Msi { upgrade_code, .. } = &mut current.owner {
                    *upgrade_code = "{33333333-3333-3333-3333-333333333333}".into();
                }
            }),
            ("old pending target", |plan, _| {
                plan.next_receipt.owner = plan.receipt.owner.clone();
                plan.next_receipt.install_id = plan.receipt.install_id.clone();
            }),
            ("current product", |_, current| {
                if let InstallOwner::Msi { product_code, .. } = &mut current.owner {
                    *product_code = "{33333333-3333-3333-3333-333333333333}".into();
                }
            }),
            ("next root id", |plan, _| {
                plan.next_receipt.root_identity.file_id += 1
            }),
            ("current root id", |_, current| {
                current.root_identity.file_id += 1
            }),
            ("next root", |plan, _| {
                plan.next_receipt.install_root = plan.next_receipt.install_root.join("other")
            }),
            ("current root", |_, current| {
                current.install_root = current.install_root.join("other")
            }),
            ("nonincreasing version", |plan, _| {
                plan.next_receipt.version = plan.receipt.version.clone()
            }),
            ("current version", |_, current| {
                current.version = "0.8.1".into()
            }),
            ("missing package", |plan, _| plan.msi_package = None),
            ("missing target hash", |plan, _| {
                plan.next_receipt.source_package_sha256.clear()
            }),
            ("multi-hop descendant", |_, current| {
                current.install_id = "33333333-3333-3333-3333-333333333333".into();
                if let InstallOwner::Msi { product_code, .. } = &mut current.owner {
                    *product_code = format!("{{{}}}", current.install_id);
                }
                current.version = "0.9.0".into();
            }),
        ];
        for (label, change) in cases {
            let mut changed_plan = plan.clone();
            let mut changed_current = current.clone();
            change(&mut changed_plan, &mut changed_current);
            assert!(
                verify_control_owner(
                    &changed_plan,
                    &changed_current,
                    &MaintenanceState::Succeeded
                )
                .is_err(),
                "{label}"
            );
        }
        for state in [
            MaintenanceState::RolledBack,
            MaintenanceState::Failed,
            MaintenanceState::Prepared,
        ] {
            assert!(
                verify_control_owner(&plan, &current, &state).is_err(),
                "{state:?}"
            );
        }
        assert!(verify_control_owner(&plan, &plan.receipt, &MaintenanceState::RolledBack).is_ok());
    }
    #[test]
    fn msi_owner_transition_requires_sealed_plan_and_exact_current_target() {
        let (prepared, current) = msi_owner_transition_fixture();
        let plan = load_plan(&prepared).unwrap();
        let mut discovered = current.clone();
        discovered.files[0].sha256 = archive::digest(b"new");
        assert!(msi::verify_target_receipt(&plan.next_receipt, &discovered).is_ok());
        let seal = prepared.plan_path.parent().unwrap().join("recovery.bin");
        std::fs::write(&seal, b"untrusted ownership seal").unwrap();
        let controls = capture_controls(&current, None).unwrap();
        assert!(
            controls
                .files
                .iter()
                .all(|file| file.path != prepared.plan_path)
        );
        assert_eq!(std::fs::read(&seal).unwrap(), b"untrusted ownership seal");
    }
    fn fixture(selection: DataSelection) -> (AuthenticatedUninstall, PathBuf, PathBuf) {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .join(".scratch/installation-20261026/test-temp")
            .join(format!("uninstall-{}", uuid::Uuid::new_v4()));
        let app = root.join("application");
        let data = root.join("library");
        std::fs::create_dir_all(app.join("support")).unwrap();
        std::fs::create_dir(&data).unwrap();
        let app_identity =
            create_new_file(&app.join("runtime.dll"), b"application fixture").unwrap();
        let root_identity = runtime::handle_identity(
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
            root_identity,
            version: env!("CARGO_PKG_VERSION").into(),
            source_package_sha256: "a".repeat(64),
            files: vec![OwnedFile {
                relative_path: "runtime.dll".into(),
                role: FileRole::Runtime,
                size: app_identity.size,
                sha256: app_identity.sha256,
            }],
        };
        create_new_file(
            &app.join(RECEIPT_PATH),
            &serde_json::to_vec(&receipt).unwrap(),
        )
        .unwrap();
        std::fs::write(data.join("studyapp.lock"), b"").unwrap();
        let mut engine = studyapp_core::Engine::open(&data.join("studyapp.sqlite3"), 1000).unwrap();
        let mut settings = engine.snapshot().settings;
        settings.interval_minutes = 25;
        engine
            .apply(studyapp_core::Command::SaveSettings(settings), 1000)
            .unwrap();
        let original = data.join("original.md");
        std::fs::write(&original, b"original content").unwrap();
        engine
            .apply(studyapp_core::Command::Import(vec![original]), 1001)
            .unwrap();
        drop(engine);
        std::fs::write(
            data.join("sync-config.json"),
            b"fixture encrypted credential input",
        )
        .unwrap();
        std::fs::write(data.join("backup.sqlite3"), b"unknown backup").unwrap();
        let context = runtime::DiscoveryContext {
            default_library: data.clone(),
            location_file: None,
            explicit_library: Some(data.clone()),
        };
        let discovery = runtime::discover_library(&context);
        let preview = preview_uninstall(
            &receipt,
            selection,
            discovery.verified_database,
            discovery.configuration_files,
        )
        .unwrap();
        let mut confirmation = UninstallConfirmation::new(preview);
        confirmation.confirm_selection();
        if selection.database {
            confirmation.confirm_database_review().unwrap();
            confirmation.confirm_database_final("删除数据库").unwrap();
        }
        let intent = prepare_uninstall(confirmation.finish().unwrap(), &receipt, &context).unwrap();
        (intent, app, data)
    }
    #[test]
    fn maintenance_uninstall_configuration_rejects_post_confirmation_hardlinks() {
        for name in [
            "studyapp.sqlite3",
            "studyapp.sqlite3-wal",
            "studyapp.sqlite3-shm",
            "studyapp.sqlite3-journal",
        ] {
            let (intent, _, data) = fixture(DataSelection {
                configuration: true,
                database: false,
            });
            let path = data.join(name);
            if !path.exists() {
                std::fs::write(&path, b"sentinel-sidecar").unwrap();
            }
            let alias = data.join("unknown-alias");
            std::fs::hard_link(&path, &alias).unwrap();
            let before = std::fs::read(&alias).unwrap();
            let status = apply_authenticated_uninstall(intent).unwrap();
            assert_eq!(status.state, MaintenanceState::Failed, "{name}");
            assert_eq!(status.code, "application_removed_data_retained");
            assert_eq!(std::fs::read(&alias).unwrap(), before, "{name}");
            assert!(data.join("sync-config.json").exists());
        }
    }
    #[test]
    fn maintenance_uninstall_control_cleanup_removes_empty_root_and_preserves_swaps() {
        let (intent, app, _) = fixture(DataSelection::default());
        let status = apply_authenticated_uninstall(intent).unwrap();
        assert_eq!(status.state, MaintenanceState::Succeeded);
        assert!(
            !app.exists(),
            "empty application directory should be removed"
        );
        let (intent, app, _) = fixture(DataSelection::default());
        let _lock = installation_lock(&app).unwrap();
        drop(_lock);
        let controls = capture_controls(&intent.receipt, None).unwrap();
        let lock = app.join("support/maintenance.lock");
        std::fs::rename(&lock, app.join("unknown-old-lock")).unwrap();
        std::fs::write(&lock, b"unknown replacement").unwrap();
        let mut retained = vec![];
        cleanup_controls(controls, &mut retained);
        assert_eq!(std::fs::read(&lock).unwrap(), b"unknown replacement");
        assert!(retained.contains(&lock));
    }
    #[test]
    fn maintenance_uninstall_completed_controls_keep_unknown_transaction_neighbor() {
        let (prepared, app) = super::super::transaction_tests::fixture();
        assert_eq!(
            apply_with_health(&prepared, |_| Ok(()), |_, _| Ok(()))
                .unwrap()
                .state,
            MaintenanceState::Succeeded
        );
        let stage = prepared.plan_path.parent().unwrap();
        std::fs::write(stage.join("unknown.txt"), b"unknown neighbor").unwrap();
        let receipt = runtime::read_portable_receipt(&app.join("WW-StudyApp.exe")).unwrap();
        let mut retained = vec![];
        let controls = remove_portable(&receipt, &mut retained).unwrap();
        cleanup_controls(controls, &mut retained);
        assert!(!prepared.plan_path.exists());
        assert!(!stage.join("completed.bin").exists());
        assert!(!app.join("support").exists());
        assert_eq!(
            std::fs::read(stage.join("unknown.txt")).unwrap(),
            b"unknown neighbor"
        );
        assert_eq!(
            std::fs::read(app.join("user-notes.md")).unwrap(),
            b"original-notes"
        );
        assert!(retained.contains(&stage.to_owned()));
    }
    #[test]
    fn maintenance_uninstall_all_four_choices_preserve_originals_and_unknown_neighbors() {
        for configuration in [false, true] {
            for database in [false, true] {
                let (intent, app, data) = fixture(DataSelection {
                    configuration,
                    database,
                });
                let before = std::fs::read(data.join("studyapp.sqlite3")).unwrap();
                let status = apply_authenticated_uninstall(intent).unwrap();
                assert_eq!(
                    status.state,
                    MaintenanceState::Succeeded,
                    "{configuration}/{database}: {status:?}"
                );
                assert!(!app.join("runtime.dll").exists());
                assert!(!app.join(RECEIPT_PATH).exists());
                assert_eq!(data.join("sync-config.json").exists(), !configuration);
                assert_eq!(data.join("studyapp.sqlite3").exists(), !database);
                assert_eq!(
                    std::fs::read(data.join("original.md")).unwrap(),
                    b"original content"
                );
                assert_eq!(
                    std::fs::read(data.join("backup.sqlite3")).unwrap(),
                    b"unknown backup"
                );
                if !configuration && !database {
                    assert_eq!(
                        std::fs::read(data.join("studyapp.sqlite3")).unwrap(),
                        before
                    );
                }
                if configuration && !database {
                    let engine =
                        studyapp_core::Engine::open(&data.join("studyapp.sqlite3"), 1002).unwrap();
                    assert_eq!(
                        engine.snapshot().settings,
                        studyapp_core::Settings::default()
                    );
                    assert_eq!(engine.snapshot().notes.len(), 1);
                }
            }
        }
    }
    #[test]
    fn maintenance_uninstall_data_lock_or_changed_identity_preserves_data_after_app_removal() {
        for busy in [false, true] {
            let (intent, app, data) = fixture(DataSelection {
                configuration: true,
                database: true,
            });
            let lock = if busy {
                Some(runtime::acquire_existing_library_lock(&data).unwrap())
            } else {
                std::fs::write(data.join("studyapp.sqlite3"), b"externally changed").unwrap();
                None
            };
            let before = std::fs::read(data.join("studyapp.sqlite3")).unwrap();
            let status = apply_authenticated_uninstall(intent).unwrap();
            assert_eq!(status.state, MaintenanceState::Failed);
            assert_eq!(status.code, "application_removed_data_retained");
            assert!(!app.join("runtime.dll").exists());
            assert_eq!(
                std::fs::read(data.join("studyapp.sqlite3")).unwrap(),
                before
            );
            assert!(data.join("sync-config.json").exists());
            drop(lock);
        }
    }
    #[test]
    fn maintenance_uninstall_modified_application_keeps_selected_data() {
        let (intent, app, data) = fixture(DataSelection {
            configuration: true,
            database: true,
        });
        std::fs::write(app.join("runtime.dll"), b"changed application").unwrap();
        let status = apply_authenticated_uninstall(intent).unwrap();
        assert_eq!(status.state, MaintenanceState::Failed);
        assert!(data.join("studyapp.sqlite3").exists());
        assert!(data.join("sync-config.json").exists());
        assert_eq!(
            std::fs::read(app.join("runtime.dll")).unwrap(),
            b"changed application"
        );
    }
}
