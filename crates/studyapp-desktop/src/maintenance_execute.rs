//! Explicit file transactions. All production entrypoints require verified ownership.
use crate::maintenance_runtime as runtime;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use studyapp_desktop::maintenance::*;

#[path = "maintenance_archive.rs"]
mod archive;
#[path = "maintenance_file_ops.rs"]
mod file_ops;
#[path = "maintenance_msi.rs"]
mod msi;
#[path = "maintenance_recovery.rs"]
mod recovery;
#[path = "maintenance_uninstall.rs"]
mod uninstall;
pub(crate) use recovery::{
    RecoveryRequest, apply_recovery, discover_recovery, recovery_snapshot, validate_recovery_source,
};
pub(crate) use uninstall::{apply_authenticated_uninstall, prepare_uninstall};
#[cfg(all(windows, feature = "ui-test"))]
#[path = "maintenance_acceptance.rs"]
mod acceptance;
#[cfg(all(windows, feature = "ui-test"))]
pub(crate) use acceptance::run_if_requested as run_acceptance_if_requested;
#[cfg(test)]
#[path = "maintenance_execute_tests.rs"]
mod transaction_tests;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalOperation {
    relative_path: String,
    original: Option<FileIdentity>,
    incoming: Option<FileIdentity>,
    staged: PathBuf,
    backup: PathBuf,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalPlan {
    schema: u32,
    receipt: InstallReceipt,
    next_receipt: InstallReceipt,
    operations: Vec<JournalOperation>,
    retained_paths: Vec<PathBuf>,
    protected_files: Vec<PathBuf>,
    protected_directories: Vec<PathBuf>,
    msi_package: Option<CapturedFile>,
}

fn create_new_file(path: &Path, bytes: &[u8]) -> Result<FileIdentity> {
    if path
        .parent()
        .is_some_and(|parent| parent.join("cleanup.json").exists())
    {
        let mut file = crate::maintenance_handoff::create_stage_file(
            path.parent().unwrap(),
            path.file_name()
                .and_then(|s| s.to_str())
                .context("暂存名称无效")?,
        )?;
        file.write_all(bytes)?;
        return file.seal();
    }
    let _guards =
        crate::storage_backend::guard_directories(path.parent().context("文件目录无效")?, false)?;
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    Ok(runtime::observe_file(path, "stage")?.identity)
}
pub(crate) fn remove_exact_maintenance_file(path: &Path, identity: &FileIdentity) -> Result<()> {
    file_ops::delete_exact(path, identity)
}
fn checked_root(receipt: &InstallReceipt) -> Result<Vec<File>> {
    validate_receipt(receipt)?;
    let guards = crate::storage_backend::guard_directories(&receipt.install_root, false)?;
    let handle = guards.last().context("缺少安装目录句柄")?;
    ensure!(
        runtime::handle_identity(handle, true)?.0 == receipt.root_identity,
        "安装目录身份已改变"
    );
    Ok(guards)
}
fn installation_lock(root: &Path) -> Result<File> {
    let path = root.join("support/maintenance.lock");
    let _guards =
        crate::storage_backend::guard_directories(path.parent().context("安装目录无效")?, false)?;
    if !path.try_exists()? {
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file.sync_all()?,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
    }
    let lock = runtime::no_delete_file_handle(&path)?;
    ensure!(
        runtime::handle_identity(&lock, false)?.1 == 1,
        "维护锁存在多个链接"
    );
    lock.try_lock().context("此安装已有维护事务")?;
    Ok(lock)
}
fn new_transaction(root: &Path) -> Result<(String, PathBuf)> {
    let _guards = crate::storage_backend::guard_directories(root, false)?;
    prune_completed_transactions(root)?;
    let count = crate::maintenance_handoff::retry_local_stages(root)?;
    ensure!(count < 8, "保留的维护记录已达上限；请先处理恢复记录");
    let id = uuid::Uuid::new_v4().simple().to_string();
    let stage = root.join(format!("WW-StudyApp-Recovery-{id}"));
    crate::maintenance_handoff::create_private_directory(&stage)?;
    crate::maintenance_handoff::initialize_local_stage(&stage)?;
    Ok((id, stage))
}
fn read_captured_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<(T, FileIdentity)> {
    let _guards =
        crate::storage_backend::guard_directories(path.parent().context("元数据目录无效")?, false)?;
    let mut file = crate::storage_backend::plain_handle(path, false, true)?;
    let (identity, links) = runtime::handle_identity(&file, false)?;
    ensure!(links == 1 && identity.size <= 1024 * 1024, "维护元数据无效");
    use std::io::Seek;
    file.rewind()?;
    let mut bytes = vec![];
    file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
    Ok((serde_json::from_slice(&bytes)?, identity))
}
/// Keep the newest result; retry only explicit captured resources in older completed journals.
fn prune_completed_transactions(root: &Path) -> Result<()> {
    let mut completed = vec![];
    for entry in std::fs::read_dir(root)?.take(1024) {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(id) = name.strip_prefix("WW-StudyApp-Recovery-") else {
            continue;
        };
        if id.len() != 32
            || !id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            continue;
        }
        let stage = entry.path();
        let Ok((status, _status_identity)) =
            read_captured_json::<MaintenanceStatus>(&stage.join("recovered.json"))
                .or_else(|_| read_captured_json(&stage.join("status.json")))
        else {
            continue;
        };
        if status.transaction_id != id
            || !matches!(
                status.state,
                MaintenanceState::Succeeded | MaintenanceState::RolledBack
            )
        {
            continue;
        }
        let Ok((plan, plan_identity)) = read_captured_json::<JournalPlan>(&stage.join("plan.json"))
        else {
            continue;
        };
        if recovery::validate_control_plan(&stage, &plan_identity).is_err() {
            continue;
        }
        if recovery::completed_state(&stage, id, &plan_identity)
            .ok()
            .flatten()
            != Some(status.state.clone())
        {
            continue;
        }
        if !same_absolute_path(&plan.receipt.install_root, root)? {
            continue;
        }
        let descriptor = PreparedMaintenance {
            _lease: None,
            transaction_id: id.into(),
            plan_path: stage.join("plan.json"),
            plan_sha256: plan_identity.sha256.clone(),
            install_root: root.into(),
            status_path: stage.join("status.json"),
            kind: if plan.msi_package.is_some() {
                PayloadKind::Msi
            } else {
                PayloadKind::PortableZip
            },
            restart_args: vec![],
            receipt: plan.receipt.clone(),
        };
        if load_plan(&descriptor).is_err() {
            continue;
        }
        let time = std::fs::metadata(if stage.join("recovered.json").exists() {
            stage.join("recovered.json")
        } else {
            descriptor.status_path
        })?
        .modified()?;
        completed.push((time, stage, plan, plan_identity));
    }
    completed.sort_by_key(|(time, ..)| *time);
    // Never discard the most recent user-visible result.
    completed.pop();
    for (_, stage, plan, _plan_identity) in completed {
        let cleanup_one = || -> Result<()> {
            let guards = crate::storage_backend::guard_directories(&stage, false)?;
            let identity =
                runtime::handle_identity(guards.last().context("维护目录无效")?, true)?.0;
            ensure!(cleanup(&plan).is_empty(), "维护备份仍需处理");
            // External MSI package lifetime belongs to its tracked staging journal.
            let journal = stage.join("journal.jsonl");
            let journal_identity = if journal.try_exists()? {
                Some(runtime::observe_file(&journal, "journal")?.identity)
            } else {
                None
            };
            let recorded = crate::maintenance_handoff::recorded_files(&stage)?;
            for entry in std::fs::read_dir(&stage)? {
                let name = entry?.file_name();
                let name = name.to_str().context("未知维护资源")?;
                ensure!(
                    name == "journal.jsonl"
                        || name == "cleanup.json"
                        || recorded.iter().any(|(known, _)| known == name),
                    "维护目录包含未知文件"
                );
            }
            if let Some(identity) = journal_identity {
                file_ops::delete_exact(&journal, &identity)?;
            }
            for (name, identity) in recorded {
                let path = stage.join(name);
                if path.try_exists()? {
                    file_ops::delete_exact(&path, &identity)?;
                }
            }
            let ownership = stage.join("cleanup.json");
            let identity_record = runtime::observe_file(&ownership, "ownership")?.identity;
            file_ops::delete_exact(&ownership, &identity_record)?;
            drop(guards);
            file_ops::delete_empty_directory(&stage, &identity)
        };
        // Leave modified/locked/unknown objects and the remaining journal for another bounded retry.
        let _ = cleanup_one();
    }
    Ok(())
}
fn descriptor(
    id: String,
    stage: &Path,
    plan: &JournalPlan,
    kind: PayloadKind,
) -> Result<PreparedMaintenance> {
    if kind == PayloadKind::PortableZip
        && plan.operations.iter().any(|op| {
            op.original.is_some()
                && plan.receipt.files.iter().any(|f| {
                    f.relative_path == op.relative_path
                        && matches!(f.role, FileRole::Application | FileRole::Runtime)
                })
        })
    {
        recovery::prepare_snapshot(stage, &plan.receipt)?;
    }
    let plan_path = stage.join("plan.json");
    let bytes = serde_json::to_vec(plan)?;
    ensure!(bytes.len() <= 1024 * 1024, "维护计划过大");
    create_new_file(&plan_path, &bytes)?;
    let prepared = PreparedMaintenance {
        _lease: Some(std::sync::Arc::new(PreparedLease {
            stages: std::iter::once(stage.to_owned())
                .chain(
                    plan.msi_package
                        .iter()
                        .filter_map(|p| p.path.parent().map(Path::to_owned)),
                )
                .collect(),
        })),
        transaction_id: id,
        plan_path,
        plan_sha256: archive::digest(&bytes),
        install_root: plan.receipt.install_root.clone(),
        status_path: stage.join("status.json"),
        kind,
        restart_args: vec![],
        receipt: plan.receipt.clone(),
    };
    recovery::seal(&prepared)?;
    Ok(prepared)
}
fn observe_union(
    receipt: &InstallReceipt,
    incoming: &PackageManifest,
) -> Result<Vec<ObservedFile>> {
    let mut names = std::collections::BTreeMap::new();
    for file in receipt.files.iter().chain(&incoming.files) {
        names.insert(
            normalize_relative_path(&file.relative_path)?,
            file.relative_path.clone(),
        );
    }
    let mut observed = vec![];
    for name in names.values() {
        let path = receipt.install_root.join(name);
        if path.try_exists()? {
            observed.push(runtime::observe_file(&path, name)?);
        }
    }
    Ok(observed)
}
pub(crate) fn prepare_portable_update(
    path: &Path,
    hash: &str,
    receipt: &InstallReceipt,
    protected: &ProtectedPaths,
    expected_version: &str,
) -> Result<PreparedMaintenance> {
    ensure!(
        receipt.owner == InstallOwner::Portable,
        "MSI 资源不可使用便携版更新"
    );
    let verified = archive::read_archive(path, hash)?;
    ensure!(
        verified.manifest.version == expected_version,
        "资源清单版本与所选发行版本不符"
    );
    ensure!(
        semver::Version::parse(&verified.manifest.version)?
            > semver::Version::parse(&receipt.version)?,
        "更新版本必须更新"
    );
    let _guards = checked_root(receipt)?;
    ensure!(
        runtime::current_payload_kind(&receipt.install_root.join("WW-StudyApp.exe"))?
            == PayloadKind::PortableZip,
        "MSI 资源不可直接替换"
    );
    let _lock = installation_lock(&receipt.install_root)?;
    let observed = observe_union(receipt, &verified.manifest)?;
    for role in [FileRole::Application, FileRole::Uninstaller] {
        let resource = receipt
            .files
            .iter()
            .find(|file| file.role == role)
            .context("恢复入口缺失；请先完善安装组件")?;
        let actual = observed
            .iter()
            .find(|file| file.relative_path == resource.relative_path)
            .context("恢复入口缺失；请先完善安装组件")?;
        ensure!(
            actual.link_count == 1
                && actual.identity.size == resource.size
                && actual.identity.sha256 == resource.sha256,
            "恢复入口已改变；请先完善安装组件"
        );
    }
    let planned = plan_upgrade(receipt, &verified.manifest, &observed, protected)?;
    let (id, stage) = new_transaction(&receipt.install_root)?;
    let mut preparation = crate::maintenance_handoff::cleanup_stage_on_error(&stage)?;
    let mut operations = vec![];
    for (index, operation) in planned.operations.into_iter().enumerate() {
        let (name, original, incoming) = match operation {
            FileOperation::Add { incoming } => {
                (incoming.relative_path, None, Some(incoming.sha256))
            }
            FileOperation::Replace {
                incoming, observed, ..
            } => (
                incoming.relative_path,
                Some(observed),
                Some(incoming.sha256),
            ),
            FileOperation::RemoveOwned { old, observed } => {
                (old.relative_path, Some(observed), None)
            }
        };
        let staged = stage.join(format!("new-{index}"));
        let backup = stage.join(format!("old-{index}"));
        let incoming = if incoming.is_some() {
            Some(create_new_file(&staged, &verified.members[&name])?)
        } else {
            None
        };
        operations.push(JournalOperation {
            relative_path: name,
            original,
            incoming,
            staged,
            backup,
        });
    }
    let mut next = receipt.clone();
    next.version = verified.manifest.version;
    next.files = verified.manifest.files;
    next.source_package_sha256 = hash.into();
    add_receipt_operation(&stage, receipt, &next, &mut operations)?;
    let prepared = descriptor(
        id,
        &stage,
        &JournalPlan {
            schema: 1,
            receipt: receipt.clone(),
            next_receipt: next,
            operations,
            retained_paths: planned
                .retained_conflicts
                .into_iter()
                .map(|p| receipt.install_root.join(p))
                .collect(),
            protected_files: protected.files.clone(),
            protected_directories: protected.directories.clone(),
            msi_package: None,
        },
        PayloadKind::PortableZip,
    )?;
    preparation.disarm();
    Ok(prepared)
}
/// Explicit same-version repair; only missing verified support/uninstaller files are added.
pub(crate) fn reconcile_current_package(
    path: &Path,
    hash: &str,
    executable: &Path,
    protected: &ProtectedPaths,
) -> Result<InstallReceipt> {
    let verified = archive::read_archive(path, hash)?;
    ensure!(
        verified.manifest.version == env!("CARGO_PKG_VERSION"),
        "完善组件必须使用当前版本安装包"
    );
    let root = executable.parent().context("安装目录无效")?;
    ensure!(
        runtime::current_payload_kind(executable)? == PayloadKind::PortableZip,
        "MSI 应通过原安装包修复"
    );
    let _guards = crate::storage_backend::guard_directories(root, false)?;
    let identity = runtime::handle_identity(_guards.last().context("安装目录无效")?, true)?.0;
    let mut existing = vec![];
    let mut missing = vec![];
    let mut resource_guards = vec![];
    for resource in &verified.manifest.files {
        let target = root.join(&resource.relative_path);
        if target.try_exists()? {
            let handle = crate::storage_backend::plain_handle(&target, false, true)?;
            let (actual, links) = runtime::handle_identity(&handle, false)?;
            ensure!(
                links == 1 && actual.size == resource.size && actual.sha256 == resource.sha256,
                "现有组件与官方当前版本不符"
            );
            resource_guards.push(handle);
            existing.push(resource.clone());
        } else {
            ensure!(
                matches!(
                    resource.role,
                    FileRole::Uninstaller | FileRole::Notice | FileRole::Manifest
                ),
                "当前主程序或运行库缺失"
            );
            missing.push(resource.clone());
        }
    }
    let receipt_path = root.join(RECEIPT_PATH);
    let previous_receipt = if receipt_path.try_exists()? {
        let receipt = runtime::read_portable_receipt(executable)?;
        ensure!(
            receipt.version == verified.manifest.version
                && receipt.files == verified.manifest.files,
            "现有安装记录需要原安装包修复"
        );
        if missing.is_empty() {
            return Ok(receipt);
        }
        Some(receipt)
    } else {
        None
    };
    let support = root.join("support");
    if !support.try_exists()? {
        std::fs::create_dir(&support)?;
    }
    let _support = crate::storage_backend::guard_directories(&support, false)?;
    let _lock = installation_lock(root)?;
    let receipt = previous_receipt.unwrap_or_else(|| InstallReceipt {
        schema: 1,
        install_id: uuid::Uuid::new_v4().to_string(),
        owner: InstallOwner::Portable,
        install_root: root.into(),
        root_identity: identity,
        version: verified.manifest.version.clone(),
        source_package_sha256: hash.into(),
        files: existing,
    });
    // Protect user data before any payload files are created.
    plan_upgrade(
        &receipt,
        &verified.manifest,
        &observe_union(&receipt, &verified.manifest)?,
        protected,
    )?;
    let (id, stage) = new_transaction(root)?;
    let mut preparation = crate::maintenance_handoff::cleanup_stage_on_error(&stage)?;
    let mut operations = vec![];
    for (index, resource) in missing.into_iter().enumerate() {
        let staged = stage.join(format!("new-{index}"));
        let incoming = create_new_file(&staged, &verified.members[&resource.relative_path])?;
        operations.push(JournalOperation {
            relative_path: resource.relative_path,
            original: None,
            incoming: Some(incoming),
            staged,
            backup: stage.join(format!("old-{index}")),
        });
    }
    let next = InstallReceipt {
        files: verified.manifest.files,
        source_package_sha256: hash.into(),
        ..receipt.clone()
    };
    add_receipt_operation(&stage, &receipt, &next, &mut operations)?;
    let prepared = descriptor(
        id,
        &stage,
        &JournalPlan {
            schema: 1,
            receipt,
            next_receipt: next.clone(),
            operations,
            retained_paths: vec![],
            protected_files: protected.files.clone(),
            protected_directories: protected.directories.clone(),
            msi_package: None,
        },
        PayloadKind::PortableZip,
    )?;
    preparation.disarm();
    drop(_lock);
    let status = apply_with_health(&prepared, |_| Ok(()), |_, _| Ok(()))?;
    ensure!(
        status.state == MaintenanceState::Succeeded,
        "完善组件未完成；请检查维护记录"
    );
    Ok(next)
}
fn add_receipt_operation(
    stage: &Path,
    old: &InstallReceipt,
    next: &InstallReceipt,
    operations: &mut Vec<JournalOperation>,
) -> Result<()> {
    let path = old.install_root.join(RECEIPT_PATH);
    let original = if path.try_exists()? {
        Some(runtime::observe_file(&path, RECEIPT_PATH)?.identity)
    } else {
        None
    };
    let staged = stage.join("new-receipt");
    let incoming = Some(create_new_file(&staged, &serde_json::to_vec(next)?)?);
    operations.push(JournalOperation {
        relative_path: RECEIPT_PATH.into(),
        original,
        incoming,
        staged,
        backup: stage.join("old-receipt"),
    });
    Ok(())
}
fn load_plan(prepared: &PreparedMaintenance) -> Result<JournalPlan> {
    ensure!(
        prepared.transaction_id.len() == 32
            && prepared
                .transaction_id
                .bytes()
                .all(|c| c.is_ascii_hexdigit())
            && valid_sha256(&prepared.plan_sha256),
        "维护事务引用无效"
    );
    let expected = prepared
        .install_root
        .join(format!("WW-StudyApp-Recovery-{}", prepared.transaction_id));
    ensure!(
        same_absolute_path(&prepared.plan_path, &expected.join("plan.json"))?
            && same_absolute_path(&prepared.status_path, &expected.join("status.json"))?,
        "维护计划位置无效"
    );
    let _guards = crate::storage_backend::guard_directories(&expected, false)?;
    let mut file = crate::storage_backend::plain_handle(&prepared.plan_path, false, true)?;
    let (identity, links) = runtime::handle_identity(&file, false)?;
    ensure!(
        links == 1 && identity.size <= 1024 * 1024 && identity.sha256 == prepared.plan_sha256,
        "维护计划已改变"
    );
    use std::io::Seek;
    file.rewind()?;
    let mut bytes = vec![];
    file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
    let plan: JournalPlan = serde_json::from_slice(&bytes)?;
    ensure!(
        plan.schema == 1
            && plan.receipt == prepared.receipt
            && same_absolute_path(&plan.receipt.install_root, &prepared.install_root)?
            && plan.operations.len() <= 129,
        "维护计划身份无效"
    );
    validate_receipt(&plan.next_receipt)?;
    for (index, operation) in plan.operations.iter().enumerate() {
        normalize_relative_path(&operation.relative_path)?;
        let (new_name, old_name) = if operation.relative_path == RECEIPT_PATH {
            ("new-receipt".into(), "old-receipt".into())
        } else {
            (format!("new-{index}"), format!("old-{index}"))
        };
        ensure!(
            same_absolute_path(&operation.staged, &expected.join(new_name))?
                && same_absolute_path(&operation.backup, &expected.join(old_name))?,
            "维护备份路径无效"
        );
        if operation.relative_path != RECEIPT_PATH {
            ensure!(
                plan.receipt
                    .files
                    .iter()
                    .chain(&plan.next_receipt.files)
                    .any(|f| f.relative_path == operation.relative_path),
                "资源不属于维护计划"
            );
        }
    }
    Ok(plan)
}
fn write_status(prepared: &PreparedMaintenance, status: &MaintenanceStatus) -> Result<()> {
    create_new_file(&prepared.status_path, &serde_json::to_vec(status)?)?;
    Ok(())
}
fn journal_line(journal: &mut File, event: &str, index: usize) -> Result<()> {
    serde_json::to_writer(
        &mut *journal,
        &serde_json::json!({"event":event,"index":index}),
    )?;
    journal.write_all(b"\n")?;
    journal.sync_all()?;
    Ok(())
}
fn rollback(plan: &JournalPlan) -> Vec<PathBuf> {
    let mut retained = vec![];
    for operation in plan.operations.iter().rev() {
        let target = plan.receipt.install_root.join(&operation.relative_path);
        let restore = || -> Result<()> {
            // Never discard the only valid new resource when its old backup is
            // unavailable or changed. A later explicit recovery can retry it.
            if let Some(original) = &operation.original {
                if operation.backup.try_exists()? {
                    let backup = runtime::observe_file(&operation.backup, "backup")?;
                    ensure!(
                        backup.link_count == 1 && backup.identity == *original,
                        "回滚备份已改变"
                    );
                } else {
                    ensure!(
                        runtime::observe_file(&target, &operation.relative_path)?.identity
                            == *original,
                        "回滚备份缺失"
                    );
                    return Ok(());
                }
            }
            if let Some(incoming) = &operation.incoming
                && target.try_exists()?
            {
                let current = runtime::observe_file(&target, &operation.relative_path)?;
                if current.identity == *incoming {
                    file_ops::delete_exact(&target, incoming)?;
                } else if operation.original.as_ref() != Some(&current.identity) {
                    bail!("回滚目标已改变");
                }
            }
            if let Some(original) = &operation.original {
                if operation.backup.try_exists()? {
                    file_ops::move_exact(&operation.backup, &target, original)?;
                } else {
                    ensure!(
                        runtime::observe_file(&target, &operation.relative_path)?.identity
                            == *original,
                        "回滚备份缺失"
                    );
                }
            }
            Ok(())
        };
        if let Err(error) = restore() {
            #[cfg(test)]
            eprintln!("rollback {}: {error:#}", operation.relative_path);
            let _ = error;
            retained.push(target);
            retained.push(operation.backup.clone());
        }
    }
    retained
}
fn cleanup(plan: &JournalPlan) -> Vec<PathBuf> {
    let mut retained = vec![];
    for operation in &plan.operations {
        for (path, identity) in [
            (&operation.backup, &operation.original),
            (&operation.staged, &operation.incoming),
        ] {
            if let Some(identity) = identity {
                match path.try_exists() {
                    Ok(false) => {}
                    Ok(true) => {
                        if file_ops::delete_exact(path, identity).is_err() {
                            retained.push(path.clone());
                        }
                    }
                    Err(_) => retained.push(path.clone()),
                }
            }
        }
    }
    retained
}
pub(crate) fn apply_portable_plan(prepared: &PreparedMaintenance) -> Result<MaintenanceStatus> {
    apply_with_health(
        prepared,
        |plan| {
            let executable = plan
                .next_receipt
                .files
                .iter()
                .find(|f| f.role == FileRole::Application)
                .context("资源清单缺少主程序")?;
            crate::maintenance_handoff::restart_and_wait_for_health(
                &prepared.install_root.join(&executable.relative_path),
                executable,
                &plan.next_receipt.version,
                &prepared.restart_args,
            )
        },
        |_, _| Ok(()),
    )
}
fn apply_with_health(
    prepared: &PreparedMaintenance,
    health: impl FnOnce(&JournalPlan) -> Result<()>,
    mut checkpoint: impl FnMut(&str, usize) -> Result<()>,
) -> Result<MaintenanceStatus> {
    ensure!(
        prepared.kind == PayloadKind::PortableZip,
        "错误的维护所有者"
    );
    let plan = load_plan(prepared)?;
    ensure!(
        plan.receipt.owner == InstallOwner::Portable && plan.msi_package.is_none(),
        "MSI 不可使用文件事务"
    );
    let _guards = checked_root(&plan.receipt)?;
    let _lock = installation_lock(&prepared.install_root)?;
    let stage = prepared.plan_path.parent().context("维护目录无效")?;
    let _stage_guards = crate::storage_backend::guard_directories(stage, false)?;
    let _support_guards =
        crate::storage_backend::guard_directories(&prepared.install_root.join("support"), false)?;
    ensure!(
        runtime::current_payload_kind(&prepared.install_root.join("WW-StudyApp.exe"))?
            == PayloadKind::PortableZip,
        "安装所有者已改变"
    );
    let incoming = PackageManifest {
        schema: 1,
        product: PRODUCT.into(),
        architecture: ARCHITECTURE.into(),
        version: plan.next_receipt.version.clone(),
        files: plan.next_receipt.files.clone(),
    };
    // Reapply the pure protected-path policy; path guards remain held through mutation.
    let protected = ProtectedPaths {
        files: plan.protected_files.clone(),
        directories: plan.protected_directories.clone(),
    };
    plan_upgrade(
        &plan.receipt,
        &incoming,
        &observe_union(&plan.receipt, &incoming)?,
        &protected,
    )?;
    let mut journal_owner = crate::maintenance_handoff::create_stage_file(stage, "journal.jsonl")?;
    crate::maintenance_handoff::mark_recoverable(stage)?;
    let journal = journal_owner.file_mut();
    journal_line(journal, "prepared", 0)?;
    let apply = || -> Result<()> {
        for (index, operation) in plan.operations.iter().enumerate() {
            checkpoint("before", index)?;
            journal_line(journal, "before", index)?;
            let target = prepared.install_root.join(&operation.relative_path);
            if let Some(original) = &operation.original {
                file_ops::move_exact(&target, &operation.backup, original)?;
            } else {
                ensure!(!target.try_exists()?, "新资源目标已存在");
            }
            checkpoint("backed_up", index)?;
            journal_line(journal, "backed_up", index)?;
            if let Some(incoming) = &operation.incoming {
                file_ops::move_exact(&operation.staged, &target, incoming)?;
            }
            checkpoint("installed", index)?;
            journal_line(journal, "installed", index)?;
        }
        journal_line(journal, "awaiting_health", plan.operations.len())?;
        checkpoint("health", plan.operations.len())?;
        health(&plan)?;
        journal_line(journal, "healthy", plan.operations.len())?;
        Ok(())
    };
    let mut status = MaintenanceStatus {
        transaction_id: prepared.transaction_id.clone(),
        state: MaintenanceState::Succeeded,
        version: plan.next_receipt.version.clone(),
        code: "complete".into(),
        retained_paths: plan.retained_paths.clone(),
    };
    if let Err(error) = apply() {
        #[cfg(test)]
        eprintln!("apply: {error:#}");
        let _ = error;
        let failures = rollback(&plan);
        status.version = plan.receipt.version.clone();
        status.state = if failures.is_empty() {
            MaintenanceState::RolledBack
        } else {
            MaintenanceState::Failed
        };
        status.code = if failures.is_empty() {
            "rolled_back"
        } else {
            "partial_recovery"
        }
        .into();
        status.retained_paths.extend(failures);
        journal_line(journal, &status.code, 0)?;
        if status.state == MaintenanceState::RolledBack {
            recovery::mark_completed(prepared, MaintenanceState::RolledBack)?;
        }
    } else {
        recovery::mark_completed(prepared, MaintenanceState::Succeeded)?;
        status.retained_paths.extend(cleanup(&plan));
        if !status.retained_paths.is_empty() {
            status.code = "cleanup_retained".into();
        }
    }
    write_status(prepared, &status)?;
    Ok(status)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreparedMaintenance {
    #[serde(skip)]
    _lease: Option<std::sync::Arc<PreparedLease>>,
    pub(crate) transaction_id: String,
    pub(crate) plan_path: PathBuf,
    pub(crate) plan_sha256: String,
    pub(crate) install_root: PathBuf,
    pub(crate) status_path: PathBuf,
    pub(crate) kind: PayloadKind,
    pub(crate) restart_args: Vec<String>,
    pub(crate) receipt: InstallReceipt,
}
#[derive(Debug)]
struct PreparedLease {
    stages: Vec<PathBuf>,
}
impl Drop for PreparedLease {
    fn drop(&mut self) {
        for stage in &self.stages {
            let _ = crate::maintenance_handoff::abandon_current(stage);
        }
    }
}
pub(crate) fn claim_prepared(prepared: &PreparedMaintenance, pid: u32, created: u64) -> Result<()> {
    let plan = load_plan(prepared)?;
    crate::maintenance_handoff::claim_stage(
        prepared.plan_path.parent().context("维护目录无效")?,
        pid,
        created,
    )?;
    if let Some(package) = plan.msi_package {
        crate::maintenance_handoff::claim_stage(
            package.path.parent().context("MSI目录无效")?,
            pid,
            created,
        )?;
    }
    Ok(())
}
impl PreparedMaintenance {
    pub(crate) fn status_path(&self) -> &Path {
        &self.status_path
    }
    pub(crate) fn install_root(&self) -> &Path {
        &self.install_root
    }
    pub(crate) fn with_restart_data_directory(&mut self, path: Option<&Path>) -> Result<()> {
        self.restart_args.clear();
        if let Some(path) = path {
            absolute_components(path)?;
            self.restart_args = vec![
                "--data-dir".into(),
                path.to_str().context("资料库路径编码无效")?.into(),
            ];
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MaintenanceState {
    Prepared,
    Applying,
    AwaitingHealth,
    Succeeded,
    RolledBack,
    Failed,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MaintenanceStatus {
    pub(crate) transaction_id: String,
    pub(crate) state: MaintenanceState,
    pub(crate) version: String,
    pub(crate) code: String,
    pub(crate) retained_paths: Vec<PathBuf>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CapturedFile {
    pub(crate) path: PathBuf,
    pub(crate) identity: FileIdentity,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AuthenticatedUninstall {
    pub(crate) receipt: InstallReceipt,
    pub(crate) transaction_id: String,
    pub(crate) configuration: bool,
    pub(crate) database: bool,
    pub(crate) database_path: Option<PathBuf>,
    pub(crate) database_identity: Option<FileIdentity>,
    pub(crate) configuration_files: Vec<CapturedFile>,
    pub(crate) database_files: Vec<CapturedFile>,
    pub(crate) library_path: Option<PathBuf>,
    pub(crate) location_file: Option<PathBuf>,
}

fn msi_exit_code(code: u32) -> Result<&'static str> {
    match code {
        0 => Ok("complete"),
        3010 => Ok("reboot_required"),
        1602 => bail!("安装已取消"),
        1618 => bail!("Windows Installer 正忙"),
        _ => bail!("Windows Installer 事务失败：{code}"),
    }
}

pub(crate) fn prepare_msi_update(
    path: &Path,
    hash: &str,
    receipt: &InstallReceipt,
    version: &str,
) -> Result<PreparedMaintenance> {
    ensure!(
        matches!(receipt.owner, InstallOwner::Msi { .. }) && valid_sha256(hash),
        "MSI 更新身份无效"
    );
    ensure!(
        semver::Version::parse(version)? > semver::Version::parse(&receipt.version)?,
        "更新版本必须更新"
    );
    msi::verify_registration(receipt)?;
    let _guards =
        crate::storage_backend::guard_directories(path.parent().context("MSI 目录无效")?, false)?;
    let mut file = crate::storage_backend::plain_handle(path, false, true)?;
    let (identity, links) = runtime::handle_identity(&file, false)?;
    ensure!(
        links == 1 && identity.size <= 256 * 1024 * 1024 && identity.sha256 == hash,
        "MSI 校验失败"
    );
    let product_code = msi::verify_package(path, version)?;
    let next = msi::planned_receipt(receipt, &product_code, version, hash)?;
    let _root = checked_root(receipt)?;
    let _lock = installation_lock(&receipt.install_root)?;
    let (id, stage) = new_transaction(&receipt.install_root)?;
    let mut preparation = crate::maintenance_handoff::cleanup_stage_on_error(&stage)?;
    use std::io::Seek;
    file.rewind()?;
    let mut bytes = vec![];
    file.take(256 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    let package_stage = crate::maintenance_handoff::create_download_stage()?;
    ensure!(
        !absolute_components(&package_stage)?
            .starts_with(&absolute_components(&receipt.install_root)?),
        "MSI 暂存必须位于安装目录外"
    );
    let mut package_cleanup = crate::maintenance_handoff::cleanup_stage_on_error(&package_stage)?;
    let package_path = package_stage.join("package.msi");
    let mut package_file =
        crate::maintenance_handoff::create_stage_file(&package_stage, "package.msi")?;
    package_file.write_all(&bytes)?;
    let identity = package_file.seal()?;
    ensure!(identity.sha256 == hash, "MSI 暂存校验不符");
    drop(package_file);
    let prepared = descriptor(
        id,
        &stage,
        &JournalPlan {
            schema: 1,
            receipt: receipt.clone(),
            next_receipt: next,
            operations: vec![],
            retained_paths: vec![],
            protected_files: vec![],
            protected_directories: vec![],
            msi_package: Some(CapturedFile {
                path: package_path,
                identity,
            }),
        },
        PayloadKind::Msi,
    )?;
    package_cleanup.disarm();
    preparation.disarm();
    Ok(prepared)
}
pub(crate) fn apply_msi_update(prepared: &PreparedMaintenance) -> Result<MaintenanceStatus> {
    ensure!(prepared.kind == PayloadKind::Msi, "错误的 MSI 维护模式");
    let plan = load_plan(prepared)?;
    msi::verify_registration(&plan.receipt)?;
    let package = plan.msi_package.context("MSI 维护计划缺少安装包")?;
    let package_stage = package.path.parent().context("MSI 目录无效")?;
    ensure!(
        package.path.file_name().and_then(|s| s.to_str()) == Some("package.msi")
            && package_stage
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|n| n.starts_with("WW-StudyApp-maintenance-"))
            && !absolute_components(package_stage)?
                .starts_with(&absolute_components(&prepared.install_root)?),
        "MSI 必须位于独立私有暂存目录"
    );
    let _guards = crate::storage_backend::guard_directories(
        package.path.parent().context("MSI 目录无效")?,
        false,
    )?;
    let handle = crate::storage_backend::plain_handle(&package.path, false, true)?;
    ensure!(
        runtime::handle_identity(&handle, false)?.0 == package.identity,
        "MSI 安装包已改变"
    );
    let product_code = msi::verify_package(&package.path, &plan.next_receipt.version)?;
    msi::verify_package_target(
        &plan.receipt,
        &plan.next_receipt,
        &product_code,
        &package.identity.sha256,
    )?;
    let result = msi::invoke("/i", package.path.as_os_str())?;
    let mut status = MaintenanceStatus {
        transaction_id: prepared.transaction_id.clone(),
        state: MaintenanceState::Failed,
        version: plan.receipt.version,
        code: format!("msi_{result}"),
        retained_paths: vec![],
    };
    if let Ok(code) = msi_exit_code(result) {
        if msi::verify_registration(&plan.next_receipt).is_ok() {
            status.state = MaintenanceState::Succeeded;
            status.version = plan.next_receipt.version.clone();
            status.code = code.into();
            if result == 0 {
                let restart = || -> Result<()> {
                    let mut context = runtime::DiscoveryContext::current_user()?;
                    if prepared.restart_args.len() == 2 && prepared.restart_args[0] == "--data-dir"
                    {
                        context.explicit_library = Some(PathBuf::from(&prepared.restart_args[1]));
                    }
                    let executable = prepared.install_root.join("WW-StudyApp.exe");
                    let installed = runtime::discover(&executable, &context)
                        .receipt
                        .context("安装后资源无法验证")?;
                    msi::verify_target_receipt(&plan.next_receipt, &installed)?;
                    let application = installed
                        .files
                        .iter()
                        .find(|f| f.role == FileRole::Application)
                        .context("安装后缺少主程序记录")?;
                    crate::maintenance_handoff::restart_and_wait_for_health(
                        &executable,
                        application,
                        &status.version,
                        &prepared.restart_args,
                    )
                };
                if restart().is_err() {
                    status.state = MaintenanceState::Failed;
                    status.code = "msi_installed_startup_failed".into();
                    status.retained_paths.push(prepared.install_root.clone());
                }
            }
        } else {
            status.code = "msi_postcondition_failed".into();
            status.retained_paths.push(prepared.install_root.clone());
        }
    }
    drop(handle);
    drop(_guards);
    if matches!(status.state, MaintenanceState::Succeeded)
        && crate::maintenance_handoff::cleanup_download_stage(
            package_stage,
            &package.path,
            &package.identity,
        )
        .is_err()
    {
        status.retained_paths.push(package.path.clone());
    }
    if status.state == MaintenanceState::Succeeded {
        recovery::mark_completed(prepared, MaintenanceState::Succeeded)?;
    }
    write_status(prepared, &status)?;
    Ok(status)
}

pub(crate) fn read_last_maintenance_status(executable: &Path) -> Option<MaintenanceStatus> {
    let root = executable.parent()?;
    let _guards = crate::storage_backend::guard_directories(root, false).ok()?;
    let mut candidates = vec![];
    for entry in std::fs::read_dir(root).ok()?.take(1024) {
        let entry = entry.ok()?;
        let name = entry.file_name();
        let name = name.to_str()?;
        let Some(id) = name.strip_prefix("WW-StudyApp-Recovery-") else {
            continue;
        };
        if id.len() != 32
            || !id
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            continue;
        }
        let directory = entry.path();
        let Ok(_stage_guard) = crate::storage_backend::guard_directories(&directory, false) else {
            continue;
        };
        let path = if directory.join("recovered.json").try_exists().ok()? {
            directory.join("recovered.json")
        } else {
            directory.join("status.json")
        };
        let Ok(file) = crate::storage_backend::plain_handle(&path, false, true) else {
            continue;
        };
        let metadata = file.metadata().ok()?;
        if metadata.len() > 65536 {
            continue;
        }
        let mut bytes = vec![];
        if file.take(65537).read_to_end(&mut bytes).is_err() {
            continue;
        }
        let Ok(status) = serde_json::from_slice::<MaintenanceStatus>(&bytes) else {
            continue;
        };
        if status.transaction_id == id
            && semver::Version::parse(&status.version).is_ok()
            && status.retained_paths.len() <= 512
        {
            candidates.push((metadata.modified().ok()?, status));
        }
    }
    candidates
        .into_iter()
        .max_by_key(|(time, _)| *time)
        .map(|(_, status)| status)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maintenance_executor_maps_msi_status_without_claiming_cancelled_success() {
        assert_eq!(msi_exit_code(0).unwrap(), "complete");
        assert_eq!(msi_exit_code(3010).unwrap(), "reboot_required");
        assert!(msi_exit_code(1602).is_err());
        assert!(msi_exit_code(1618).is_err());
        assert!(msi_exit_code(1603).is_err());
    }
}
