//! Recovery authority is sealed for this user and covers application files only.
use super::*;

const SEAL_NAME: &str = "recovery.bin";
fn publish(stage: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    publish_with_checkpoint(stage, name, bytes, |_| Ok(()))
}
fn publish_with_checkpoint(
    stage: &Path,
    name: &str,
    bytes: &[u8],
    mut checkpoint: impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    let destination = stage.join(name);
    if destination.try_exists()? {
        let file = crate::storage_backend::plain_handle(&destination, false, true)?;
        let mut actual = vec![];
        file.take(65537).read_to_end(&mut actual)?;
        ensure!(actual == bytes, "维护完成文件已改变，已保留未知内容");
        return Ok(());
    }
    let temporary = stage.join(format!("pending-{}", uuid::Uuid::new_v4().simple()));
    let mut file = crate::maintenance_handoff::create_stage_file(
        stage,
        temporary
            .file_name()
            .and_then(|s| s.to_str())
            .context("临时完成名称无效")?,
    )?;
    let half = bytes.len() / 2;
    file.write_all(&bytes[..half])?;
    checkpoint("partial")?;
    file.write_all(&bytes[half..])?;
    let identity = file.seal()?;
    drop(file);
    checkpoint("flushed")?;
    crate::maintenance_handoff::record_publication(stage, name, identity.clone())?;
    checkpoint("recorded")?;
    file_ops::move_exact(&temporary, &destination, &identity)?;
    checkpoint("published")?;
    Ok(())
}
pub(super) fn prepare_snapshot(stage: &Path, receipt: &InstallReceipt) -> Result<()> {
    let Some(entry) = receipt
        .files
        .iter()
        .find(|f| f.role == FileRole::Uninstaller)
    else {
        return Ok(());
    };
    if !receipt
        .files
        .iter()
        .any(|f| f.role == FileRole::Application)
    {
        return Ok(());
    }
    for resource in
        std::iter::once(entry).chain(receipt.files.iter().filter(|f| f.role == FileRole::Runtime))
    {
        let source = receipt.install_root.join(&resource.relative_path);
        let mut file = crate::storage_backend::plain_handle(&source, false, true)?;
        let (identity, links) = runtime::handle_identity(&file, false)?;
        ensure!(
            links == 1 && identity.sha256 == resource.sha256 && identity.size == resource.size,
            "恢复快照来源已改变"
        );
        use std::io::Seek;
        file.rewind()?;
        let mut bytes = vec![];
        file.take(256 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
        ensure!(bytes.len() as u64 == resource.size, "恢复快照长度不符");
        create_new_file(&stage.join(&resource.relative_path), &bytes)?;
    }
    Ok(())
}
pub(crate) fn recovery_snapshot(request: &RecoveryRequest) -> Result<PathBuf> {
    let stage = request
        .prepared
        .plan_path
        .parent()
        .context("恢复目录无效")?;
    let (prepared, plan) = unseal(stage)?;
    ensure!(
        prepared.plan_sha256 == request.prepared.plan_sha256,
        "恢复计划已改变"
    );
    for resource in plan
        .receipt
        .files
        .iter()
        .filter(|f| matches!(f.role, FileRole::Uninstaller | FileRole::Runtime))
    {
        let actual = runtime::observe_file(
            &stage.join(&resource.relative_path),
            &resource.relative_path,
        )?;
        ensure!(
            actual.link_count == 1
                && actual.identity.size == resource.size
                && actual.identity.sha256 == resource.sha256,
            "独立恢复资源已改变"
        );
    }
    Ok(stage.to_owned())
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Seal {
    schema: u32,
    transaction: String,
    root: PathBuf,
    root_identity: FileIdentity,
    plan_identity: FileIdentity,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Completion {
    schema: u32,
    transaction: String,
    plan_identity: FileIdentity,
    state: MaintenanceState,
}
pub(super) fn mark_completed(
    prepared: &PreparedMaintenance,
    state: MaintenanceState,
) -> Result<()> {
    ensure!(
        matches!(
            state,
            MaintenanceState::Succeeded | MaintenanceState::RolledBack
        ),
        "维护完成状态无效"
    );
    let identity = runtime::observe_file(&prepared.plan_path, "plan")?.identity;
    ensure!(identity.sha256 == prepared.plan_sha256, "维护计划已改变");
    let record = Completion {
        schema: 1,
        transaction: prepared.transaction_id.clone(),
        plan_identity: identity,
        state,
    };
    crate::maintenance_handoff::mark_recoverable(
        prepared.plan_path.parent().context("维护目录无效")?,
    )?;
    publish(
        prepared.plan_path.parent().context("维护目录无效")?,
        "completed.bin",
        &crypt(&serde_json::to_vec(&record)?, true)?,
    )?;
    Ok(())
}
pub(super) fn completed_state(
    stage: &Path,
    transaction: &str,
    plan_identity: &FileIdentity,
) -> Result<Option<MaintenanceState>> {
    let path = stage.join("completed.bin");
    if !path.try_exists()? {
        return Ok(None);
    }
    let mut file = crate::storage_backend::plain_handle(&path, false, true)?;
    ensure!(file.metadata()?.len() <= 65536, "完成记录过大");
    let mut bytes = vec![];
    file.read_to_end(&mut bytes)?;
    let record: Completion = serde_json::from_slice(&crypt(&bytes, false)?)?;
    ensure!(
        record.schema == 1
            && record.transaction == transaction
            && record.plan_identity == *plan_identity
            && matches!(
                record.state,
                MaintenanceState::Succeeded | MaintenanceState::RolledBack
            ),
        "完成记录未通过验证"
    );
    Ok(Some(record.state))
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecoveryRequest {
    pub(crate) prepared: PreparedMaintenance,
    pub(crate) source_receipt: InstallReceipt,
}

pub(super) fn seal(prepared: &PreparedMaintenance) -> Result<()> {
    let record = Seal {
        schema: 1,
        transaction: prepared.transaction_id.clone(),
        root: prepared.install_root.clone(),
        root_identity: prepared.receipt.root_identity.clone(),
        plan_identity: runtime::observe_file(&prepared.plan_path, "plan")?.identity,
    };
    let plaintext = serde_json::to_vec(&record)?;
    create_new_file(
        &prepared
            .plan_path
            .parent()
            .context("维护目录无效")?
            .join(SEAL_NAME),
        &crypt(&plaintext, true)?,
    )?;
    Ok(())
}
#[cfg(windows)]
fn crypt(input: &[u8], encrypt: bool) -> Result<Vec<u8>> {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
    };
    ensure!(input.len() <= 65536, "恢复记录过大");
    let context = b"WW-StudyApp/application-recovery/v1";
    let source = CRYPT_INTEGER_BLOB {
        cbData: input.len() as u32,
        pbData: input.as_ptr().cast_mut(),
    };
    let entropy = CRYPT_INTEGER_BLOB {
        cbData: context.len() as u32,
        pbData: context.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // SAFETY: bounded live input/entropy, user-scope DPAPI with all UI forbidden.
    let success = unsafe {
        if encrypt {
            CryptProtectData(
                &source,
                std::ptr::null(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &source,
                std::ptr::null_mut(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    ensure!(
        success != 0 && !output.pbData.is_null(),
        "恢复记录未通过当前用户完整性验证"
    );
    // SAFETY: DPAPI allocated this buffer; copy it before releasing with LocalFree.
    let bytes = unsafe {
        let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        LocalFree(output.pbData.cast());
        bytes
    };
    Ok(bytes)
}
#[cfg(not(windows))]
fn crypt(_: &[u8], _: bool) -> Result<Vec<u8>> {
    bail!("恢复功能仅支持 Windows")
}

fn unseal(stage: &Path) -> Result<(PreparedMaintenance, JournalPlan)> {
    let _guards = crate::storage_backend::guard_directories(stage, false)?;
    let mut file = crate::storage_backend::plain_handle(&stage.join(SEAL_NAME), false, true)?;
    ensure!(file.metadata()?.len() <= 65536, "恢复记录过大");
    let mut bytes = vec![];
    file.read_to_end(&mut bytes)?;
    let seal: Seal = serde_json::from_slice(&crypt(&bytes, false)?)?;
    ensure!(
        seal.schema == 1
            && seal.transaction.len() == 32
            && seal.transaction.bytes().all(|b| b.is_ascii_hexdigit()),
        "恢复事务无效"
    );
    ensure!(
        same_absolute_path(
            stage,
            &seal
                .root
                .join(format!("WW-StudyApp-Recovery-{}", seal.transaction))
        )?,
        "恢复位置不符"
    );
    let plan_path = stage.join("plan.json");
    ensure!(
        runtime::observe_file(&plan_path, "plan")?.identity == seal.plan_identity,
        "恢复计划已改变"
    );
    let (plan, _) = read_captured_json::<JournalPlan>(&plan_path)?;
    ensure!(
        plan.receipt.owner == InstallOwner::Portable
            && plan.msi_package.is_none()
            && plan.receipt.root_identity == seal.root_identity
            && same_absolute_path(&plan.receipt.install_root, &seal.root)?,
        "恢复所有者不符"
    );
    let prepared = PreparedMaintenance {
        _lease: None,
        transaction_id: seal.transaction,
        plan_path,
        plan_sha256: seal.plan_identity.sha256,
        install_root: seal.root,
        status_path: stage.join("status.json"),
        kind: PayloadKind::PortableZip,
        restart_args: vec![],
        receipt: plan.receipt.clone(),
    };
    let plan = load_plan(&prepared)?;
    Ok((prepared, plan))
}
pub(super) fn validate_control_plan(stage: &Path, identity: &FileIdentity) -> Result<()> {
    let _guards = crate::storage_backend::guard_directories(stage, false)?;
    let mut file = crate::storage_backend::plain_handle(&stage.join(SEAL_NAME), false, true)?;
    ensure!(file.metadata()?.len() <= 65536, "控制记录过大");
    let mut bytes = vec![];
    file.read_to_end(&mut bytes)?;
    let seal: Seal = serde_json::from_slice(&crypt(&bytes, false)?)?;
    ensure!(
        seal.schema == 1
            && seal.plan_identity == *identity
            && same_absolute_path(
                stage,
                &seal
                    .root
                    .join(format!("WW-StudyApp-Recovery-{}", seal.transaction))
            )?,
        "维护控制计划已改变"
    );
    Ok(())
}

pub(crate) fn discover_recovery(executable: &Path) -> Result<Option<RecoveryRequest>> {
    let root = executable.parent().context("应用目录无效")?;
    let snapshot_directory = root
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with("WW-StudyApp-Recovery-"));
    let package_entry = executable
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.eq_ignore_ascii_case("WW-StudyApp.exe")
                || name.eq_ignore_ascii_case("Uninstall.exe")
        });
    if !package_entry {
        ensure!(!snapshot_directory, "恢复目录入口名称无效");
        return Ok(None);
    }
    let _guards = crate::storage_backend::guard_directories(root, false)?;
    if snapshot_directory {
        let (prepared, plan) = unseal(root)?;
        let request = RecoveryRequest {
            prepared,
            source_receipt: plan.receipt,
        };
        validate_recovery_source(&request, executable)?;
        return Ok(Some(request));
    }
    if runtime::current_payload_kind(executable)? != PayloadKind::PortableZip {
        return Ok(None);
    }
    let mut pending = None;
    let mut count = 0;
    for entry in std::fs::read_dir(root)?.take(1024) {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with("WW-StudyApp-Recovery-") {
            continue;
        }
        count += 1;
        ensure!(count <= 8, "未完成维护数量超过检查范围");
        let stage = entry.path();
        if !stage.join("journal.jsonl").try_exists()? {
            continue;
        }
        let (prepared, plan) = unseal(&stage)?;
        let _root = checked_root(&plan.receipt)?;
        let identity = runtime::observe_file(&prepared.plan_path, "plan")?.identity;
        if completed_state(&stage, &prepared.transaction_id, &identity)?.is_some() {
            continue;
        }
        let basename = executable
            .file_name()
            .and_then(|s| s.to_str())
            .context("恢复入口无效")?;
        let observed = runtime::observe_file(executable, basename)?;
        let source_receipt = [&plan.receipt, &plan.next_receipt]
            .into_iter()
            .find(|receipt| {
                receipt.files.iter().any(|f| {
                    f.relative_path.eq_ignore_ascii_case(basename)
                        && matches!(f.role, FileRole::Application | FileRole::Uninstaller)
                        && f.size == observed.identity.size
                        && f.sha256 == observed.identity.sha256
                })
            })
            .context("恢复入口与已验证资源不符")?
            .clone();
        ensure!(pending.is_none(), "存在多个未完成维护事务；所有文件已保留");
        pending = Some(RecoveryRequest {
            prepared,
            source_receipt,
        });
    }
    Ok(pending)
}
pub(crate) fn validate_recovery_source(request: &RecoveryRequest, source: &Path) -> Result<()> {
    let snapshot = recovery_snapshot(request)?;
    let (_, plan) = unseal(&snapshot)?;
    ensure!(
        request.source_receipt == plan.receipt || request.source_receipt == plan.next_receipt,
        "恢复入口来源无效"
    );
    let parent = source.parent().context("恢复来源无效")?;
    ensure!(
        same_absolute_path(parent, &snapshot)?
            || same_absolute_path(parent, &plan.receipt.install_root)?,
        "恢复来源目录不符"
    );
    if same_absolute_path(parent, &snapshot)? {
        ensure!(
            source.file_name().and_then(|s| s.to_str()) == Some("Uninstall.exe")
                && request.source_receipt == plan.receipt,
            "恢复快照入口无效"
        );
    }
    let name = source
        .file_name()
        .and_then(|s| s.to_str())
        .context("恢复入口名无效")?;
    let file = request
        .source_receipt
        .files
        .iter()
        .find(|f| {
            f.relative_path == name
                && matches!(f.role, FileRole::Application | FileRole::Uninstaller)
        })
        .context("恢复入口未登记")?;
    let actual = runtime::observe_file(source, name)?;
    ensure!(
        actual.link_count == 1
            && actual.identity.size == file.size
            && actual.identity.sha256 == file.sha256,
        "恢复入口已改变"
    );
    Ok(())
}

pub(crate) fn apply_recovery(request: &RecoveryRequest) -> Result<MaintenanceStatus> {
    let stage = request
        .prepared
        .plan_path
        .parent()
        .context("恢复目录无效")?;
    let (prepared, plan) = unseal(stage)?;
    ensure!(
        prepared.transaction_id == request.prepared.transaction_id
            && prepared.plan_sha256 == request.prepared.plan_sha256
            && prepared.receipt == request.prepared.receipt,
        "恢复请求已改变"
    );
    ensure!(
        request.source_receipt == plan.receipt || request.source_receipt == plan.next_receipt,
        "恢复入口来源无效"
    );
    let _root = checked_root(&plan.receipt)?;
    ensure!(
        runtime::current_payload_kind(&prepared.install_root.join("WW-StudyApp.exe"))?
            == PayloadKind::PortableZip,
        "MSI 不能进行文件回滚"
    );
    let _lock = installation_lock(&prepared.install_root)?;
    crate::maintenance_handoff::claim_recovery_stage(stage)?;
    let _stage = crate::storage_backend::guard_directories(stage, false)?;
    let _support =
        crate::storage_backend::guard_directories(&prepared.install_root.join("support"), false)?;
    let mut journal =
        crate::storage_backend::plain_handle(&stage.join("journal.jsonl"), false, true)?;
    ensure!(journal.metadata()?.len() <= 65536, "恢复日志过大");
    let mut bytes = vec![];
    journal.read_to_end(&mut bytes)?;
    let lines = bytes.split(|b| *b == b'\n').collect::<Vec<_>>();
    ensure!(lines.len() <= 512, "恢复日志条目过多");
    let mut count = 0;
    for (index, line) in lines.iter().enumerate() {
        if line.is_empty() {
            continue;
        }
        let parsed = serde_json::from_slice::<serde_json::Value>(line);
        if index + 1 == lines.len() && parsed.is_err() {
            break;
        } // torn final write grants no additional authority
        let value = parsed?;
        let event = value["event"].as_str().context("恢复事件无效")?;
        let step = value["index"].as_u64().context("恢复事件索引无效")?;
        ensure!(
            step <= plan.operations.len() as u64
                && [
                    "prepared",
                    "before",
                    "backed_up",
                    "installed",
                    "awaiting_health",
                    "healthy",
                    "rolled_back",
                    "partial_recovery"
                ]
                .contains(&event),
            "恢复日志不受支持"
        );
        if count == 0 {
            ensure!(event == "prepared" && step == 0, "恢复日志缺少起始记录");
        }
        count += 1;
    }
    for operation in &plan.operations {
        let target = absolute_components(&prepared.install_root.join(&operation.relative_path))?;
        for path in &plan.protected_files {
            ensure!(
                target != absolute_components(path)?,
                "恢复路径与用户文件冲突"
            );
        }
        for path in &plan.protected_directories {
            ensure!(
                !target.starts_with(&absolute_components(path)?),
                "恢复路径位于资料库"
            );
        }
    }
    let plan_identity = runtime::observe_file(&prepared.plan_path, "plan")?.identity;
    let terminal = completed_state(stage, &prepared.transaction_id, &plan_identity)?;
    if terminal == Some(MaintenanceState::Succeeded) {
        for resource in &plan.next_receipt.files {
            let observed = runtime::observe_file(
                &prepared.install_root.join(&resource.relative_path),
                &resource.relative_path,
            )?;
            ensure!(
                observed.link_count == 1
                    && observed.identity.size == resource.size
                    && observed.identity.sha256 == resource.sha256,
                "已完成更新的应用资源已改变"
            );
        }
        ensure!(
            runtime::read_portable_receipt(&prepared.install_root.join("WW-StudyApp.exe"))?
                == plan.next_receipt,
            "已完成更新的安装记录已改变"
        );
        let mut status = MaintenanceStatus {
            transaction_id: prepared.transaction_id,
            version: plan.next_receipt.version.clone(),
            state: MaintenanceState::Succeeded,
            code: "interrupted_update_committed".into(),
            retained_paths: cleanup(&plan),
        };
        if publish(stage, "recovered.json", &serde_json::to_vec(&status)?).is_err() {
            status.retained_paths.push(stage.join("recovered.json"));
        }
        return Ok(status);
    }
    let failures = rollback(&plan);
    let complete = failures.is_empty();
    let mut status = MaintenanceStatus {
        transaction_id: prepared.transaction_id,
        version: plan.receipt.version.clone(),
        state: if complete {
            MaintenanceState::RolledBack
        } else {
            MaintenanceState::Failed
        },
        code: if complete {
            "interrupted_update_recovered"
        } else {
            "partial_recovery"
        }
        .into(),
        retained_paths: failures,
    };
    if complete {
        if terminal.is_none() {
            mark_completed(&request.prepared, MaintenanceState::RolledBack)?;
        }
        status.retained_paths.extend(cleanup(&plan));
    }
    let result = if complete {
        "recovered.json"
    } else {
        "recovery-partial.json"
    };
    if publish(stage, result, &serde_json::to_vec(&status)?).is_err() {
        status.retained_paths.push(stage.join(result));
    }
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maintenance_recovery_development_entries_skip_discovery_without_owner_relaxation() {
        let root = PathBuf::from(r"C:\WW-StudyApp-nonexistent-startup-fixture");
        for name in ["studyapp.exe", "STUDYAPP.EXE", "studyapp-a1b2c3.exe"] {
            let executable = root.join(name);
            assert!(discover_recovery(&executable).unwrap().is_none(), "{name}");
            assert!(runtime::matching_msi_owner(&executable, &[]).is_err());
        }
        for name in [
            "WW-StudyApp.exe",
            "ww-studyapp.EXE",
            "Uninstall.exe",
            "UNINSTALL.EXE",
        ] {
            assert!(
                discover_recovery(&root.join(name)).is_err(),
                "canonical name must retain real discovery: {name}"
            );
        }
        let snapshot = root.join("WW-StudyApp-Recovery-0123456789abcdef0123456789abcdef");
        assert!(
            discover_recovery(&snapshot.join("renamed.exe")).is_err(),
            "renamed snapshot must not enter normal startup"
        );
    }
    #[test]
    fn maintenance_recovery_terminal_publication_retries_partial_and_flushed_records() {
        for name in ["completed.bin", "recovered.json"] {
            for boundary in ["partial", "flushed", "recorded"] {
                let (prepared, _) = super::super::transaction_tests::fixture();
                let stage = prepared.plan_path.parent().unwrap();
                let bytes = if name == "completed.bin" {
                    crypt(
                        &serde_json::to_vec(&Completion {
                            schema: 1,
                            transaction: prepared.transaction_id.clone(),
                            plan_identity: runtime::observe_file(&prepared.plan_path, "plan")
                                .unwrap()
                                .identity,
                            state: MaintenanceState::RolledBack,
                        })
                        .unwrap(),
                        true,
                    )
                    .unwrap()
                } else {
                    serde_json::to_vec(&MaintenanceStatus {
                        transaction_id: prepared.transaction_id.clone(),
                        version: "0.7.3".into(),
                        state: MaintenanceState::RolledBack,
                        code: "interrupted_update_recovered".into(),
                        retained_paths: vec![],
                    })
                    .unwrap()
                };
                assert!(
                    publish_with_checkpoint(stage, name, &bytes, |at| if at == boundary {
                        bail!("injected publication interruption")
                    } else {
                        Ok(())
                    })
                    .is_err()
                );
                assert!(!stage.join(name).exists());
                publish(stage, name, &bytes).unwrap();
                assert_eq!(std::fs::read(stage.join(name)).unwrap(), bytes);
                if name == "completed.bin" {
                    assert_eq!(
                        completed_state(
                            stage,
                            &prepared.transaction_id,
                            &runtime::observe_file(&prepared.plan_path, "plan")
                                .unwrap()
                                .identity
                        )
                        .unwrap(),
                        Some(MaintenanceState::RolledBack)
                    );
                }
            }
        }
        let (prepared, _) = super::super::transaction_tests::fixture();
        let stage = prepared.plan_path.parent().unwrap();
        std::fs::write(stage.join("recovered.json"), b"unknown replacement").unwrap();
        assert!(publish(stage, "recovered.json", b"owned result").is_err());
        assert_eq!(
            std::fs::read(stage.join("recovered.json")).unwrap(),
            b"unknown replacement"
        );
    }
    #[test]
    fn maintenance_recovery_seal_rejects_modified_ciphertext() {
        let encrypted = crypt(b"fixture", true).unwrap();
        assert_eq!(crypt(&encrypted, false).unwrap(), b"fixture");
        let mut changed = encrypted;
        let index = changed.len() / 2;
        changed[index] ^= 1;
        assert!(crypt(&changed, false).is_err());
    }
    #[test]
    fn maintenance_recovery_remains_reachable_through_either_verified_entrypoint() {
        for removed in [
            "WW-StudyApp.exe",
            "Uninstall.exe",
            "vcruntime140.dll",
            "vcruntime140_1.dll",
        ] {
            let (seed, root) = super::super::transaction_tests::fixture();
            drop(seed);
            let mut receipt =
                runtime::read_portable_receipt(&root.join("WW-StudyApp.exe")).unwrap();
            receipt.files.clear();
            let (id, stage) = new_transaction(&root).unwrap();
            let mut next = receipt.clone();
            next.version = "0.8.0".into();
            let mut operations = vec![];
            for (index, (name, role)) in [
                ("WW-StudyApp.exe", FileRole::Application),
                ("Uninstall.exe", FileRole::Uninstaller),
                ("vcruntime140.dll", FileRole::Runtime),
                ("vcruntime140_1.dll", FileRole::Runtime),
            ]
            .into_iter()
            .enumerate()
            {
                let original = create_new_file(&root.join(name), b"old-entry").unwrap();
                let staged = stage.join(format!("new-{index}"));
                let incoming = create_new_file(&staged, b"new-entry").unwrap();
                receipt.files.push(OwnedFile {
                    relative_path: name.into(),
                    role,
                    size: original.size,
                    sha256: original.sha256.clone(),
                });
                next.files.push(OwnedFile {
                    relative_path: name.into(),
                    role,
                    size: incoming.size,
                    sha256: incoming.sha256.clone(),
                });
                operations.push(JournalOperation {
                    relative_path: name.into(),
                    original: Some(original),
                    incoming: Some(incoming),
                    staged,
                    backup: stage.join(format!("old-{index}")),
                });
            }
            std::fs::write(
                root.join(RECEIPT_PATH),
                serde_json::to_vec(&receipt).unwrap(),
            )
            .unwrap();
            add_receipt_operation(&stage, &receipt, &next, &mut operations).unwrap();
            let plan = JournalPlan {
                schema: 1,
                receipt,
                next_receipt: next,
                operations,
                retained_paths: vec![],
                protected_files: vec![],
                protected_directories: vec![],
                msi_package: None,
            };
            let prepared = descriptor(id, &stage, &plan, PayloadKind::PortableZip).unwrap();
            let mut journal = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(stage.join("journal.jsonl"))
                .unwrap();
            journal_line(&mut journal, "prepared", 0).unwrap();
            drop(journal);
            for operation in &plan.operations {
                if operation.relative_path == RECEIPT_PATH {
                    break;
                }
                file_ops::move_exact(
                    &root.join(&operation.relative_path),
                    &operation.backup,
                    operation.original.as_ref().unwrap(),
                )
                .unwrap();
                if operation.relative_path == removed {
                    break;
                }
                file_ops::move_exact(
                    &operation.staged,
                    &root.join(&operation.relative_path),
                    operation.incoming.as_ref().unwrap(),
                )
                .unwrap();
            }
            let entry = if removed.ends_with(".dll") {
                stage.join("Uninstall.exe")
            } else if removed == "WW-StudyApp.exe" {
                root.join("Uninstall.exe")
            } else {
                root.join("WW-StudyApp.exe")
            };
            let request = discover_recovery(&entry).unwrap().unwrap();
            validate_recovery_source(&request, &entry).unwrap();
            assert_eq!(
                std::fs::read(
                    recovery_snapshot(&request)
                        .unwrap()
                        .join("vcruntime140.dll")
                )
                .unwrap(),
                b"old-entry"
            );
            assert_eq!(request.prepared.transaction_id, prepared.transaction_id);
            let result = apply_recovery(&request).unwrap();
            assert_eq!(result.state, MaintenanceState::RolledBack);
            assert_eq!(std::fs::read(root.join(removed)).unwrap(), b"old-entry");
        }
    }
    #[test]
    fn maintenance_recovery_retains_new_resource_when_backup_was_replaced() {
        let (prepared, root) = super::super::transaction_tests::fixture();
        let plan = load_plan(&prepared).unwrap();
        let stage = prepared.plan_path.parent().unwrap();
        let mut journal = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(stage.join("journal.jsonl"))
            .unwrap();
        journal_line(&mut journal, "prepared", 0).unwrap();
        drop(journal);
        let operation = &plan.operations[0];
        let target = root.join(&operation.relative_path);
        file_ops::move_exact(
            &target,
            &operation.backup,
            operation.original.as_ref().unwrap(),
        )
        .unwrap();
        file_ops::move_exact(
            &operation.staged,
            &target,
            operation.incoming.as_ref().unwrap(),
        )
        .unwrap();
        std::fs::write(&operation.backup, b"unknown replacement").unwrap();
        let result = apply_recovery(&RecoveryRequest {
            prepared: prepared.clone(),
            source_receipt: plan.receipt,
        })
        .unwrap();
        assert_eq!(result.state, MaintenanceState::Failed);
        assert_eq!(std::fs::read(&target).unwrap(), b"new-runtime");
        assert_eq!(
            std::fs::read(&operation.backup).unwrap(),
            b"unknown replacement"
        );
    }
    #[test]
    fn maintenance_recovery_finishes_sealed_commit_after_interrupted_cleanup() {
        let (prepared, root) = super::super::transaction_tests::fixture();
        let plan = load_plan(&prepared).unwrap();
        let mut journal = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(prepared.plan_path.parent().unwrap().join("journal.jsonl"))
            .unwrap();
        journal_line(&mut journal, "prepared", 0).unwrap();
        drop(journal);
        for operation in &plan.operations {
            let target = root.join(&operation.relative_path);
            if let Some(original) = &operation.original {
                file_ops::move_exact(&target, &operation.backup, original).unwrap();
            }
            if let Some(incoming) = &operation.incoming {
                file_ops::move_exact(&operation.staged, &target, incoming).unwrap();
            }
        }
        mark_completed(&prepared, MaintenanceState::Succeeded).unwrap();
        let first = &plan.operations[0];
        file_ops::delete_exact(&first.backup, first.original.as_ref().unwrap()).unwrap();
        let result = apply_recovery(&RecoveryRequest {
            prepared,
            source_receipt: plan.next_receipt,
        })
        .unwrap();
        assert_eq!(result.state, MaintenanceState::Succeeded);
        assert_eq!(
            std::fs::read(root.join("runtime.dll")).unwrap(),
            b"new-runtime"
        );
    }
    #[test]
    fn maintenance_recovery_rolls_back_interrupted_boundaries_without_live_userdata_authority() {
        for stop in 0..8 {
            let (prepared, root) = super::super::transaction_tests::fixture();
            let plan = load_plan(&prepared).unwrap();
            let mut journal = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(prepared.plan_path.parent().unwrap().join("journal.jsonl"))
                .unwrap();
            journal_line(&mut journal, "prepared", 0).unwrap();
            for (index, operation) in plan.operations.iter().enumerate() {
                if index * 2 >= stop {
                    break;
                }
                journal_line(&mut journal, "before", index).unwrap();
                let target = root.join(&operation.relative_path);
                if let Some(original) = &operation.original {
                    file_ops::move_exact(&target, &operation.backup, original).unwrap();
                }
                if index * 2 + 1 >= stop {
                    break;
                }
                journal_line(&mut journal, "backed_up", index).unwrap();
                if let Some(incoming) = &operation.incoming {
                    file_ops::move_exact(&operation.staged, &target, incoming).unwrap();
                }
            }
            drop(journal);
            let status = apply_recovery(&RecoveryRequest {
                prepared: prepared.clone(),
                source_receipt: plan.receipt,
            })
            .unwrap();
            assert_eq!(
                status.state,
                MaintenanceState::RolledBack,
                "boundary {stop}"
            );
            assert_eq!(
                std::fs::read(root.join("runtime.dll")).unwrap(),
                b"old-runtime"
            );
            assert_eq!(
                std::fs::read(root.join("retired.dll")).unwrap(),
                b"old-retired"
            );
            assert_eq!(
                std::fs::read(root.join("user-notes.md")).unwrap(),
                b"original-notes"
            );
        }
    }
}
