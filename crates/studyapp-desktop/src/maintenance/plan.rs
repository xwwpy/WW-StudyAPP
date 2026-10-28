use super::*;
use anyhow::{Result, ensure};
use std::{collections::BTreeMap, path::PathBuf};

fn check_protected(
    root: &std::path::Path,
    files: &[OwnedFile],
    protected: &ProtectedPaths,
) -> Result<()> {
    let root = absolute_components(root)?;
    for file in files {
        let mut target = root.clone();
        target.extend(
            normalize_relative_path(&file.relative_path)?
                .split('/')
                .map(str::to_owned),
        );
        for path in &protected.files {
            ensure!(
                target != absolute_components(path)?,
                "应用资源与受保护文件冲突"
            );
        }
        for path in &protected.directories {
            ensure!(
                !target.starts_with(&absolute_components(path)?),
                "应用资源位于受保护目录"
            );
        }
    }
    Ok(())
}
pub fn plan_upgrade(
    receipt: &InstallReceipt,
    incoming: &PackageManifest,
    observed: &[ObservedFile],
    protected: &ProtectedPaths,
) -> Result<UpgradePlan> {
    validate_receipt(receipt)?;
    validate_manifest(incoming)?;
    ensure!(
        receipt.owner == InstallOwner::Portable,
        "此安装不能直接替换应用文件"
    );
    check_protected(&receipt.install_root, &receipt.files, protected)?;
    check_protected(&receipt.install_root, &incoming.files, protected)?;
    let old: BTreeMap<_, _> = receipt
        .files
        .iter()
        .map(|f| Ok((normalize_relative_path(&f.relative_path)?, f)))
        .collect::<Result<_>>()?;
    let mut actual = BTreeMap::new();
    for file in observed {
        let key = normalize_relative_path(&file.relative_path)?;
        ensure!(actual.insert(key, file).is_none(), "观测路径冲突");
    }
    let new: BTreeMap<_, _> = incoming
        .files
        .iter()
        .map(|f| Ok((normalize_relative_path(&f.relative_path)?, f)))
        .collect::<Result<_>>()?;
    let mut plan = UpgradePlan {
        install_id: receipt.install_id.clone(),
        from_version: receipt.version.clone(),
        to_version: incoming.version.clone(),
        operations: vec![],
        retained_conflicts: vec![],
    };
    for file in &incoming.files {
        let key = normalize_relative_path(&file.relative_path)?;
        match (old.get(&key), actual.get(&key)) {
            (Some(old), Some(actual)) => {
                ensure!(
                    !actual.is_reparse && actual.link_count == 1,
                    "应用资源不是独立普通文件"
                );
                ensure!(
                    actual.identity.size == old.size && actual.identity.sha256 == old.sha256,
                    "应用资源已修改，无法安全更新"
                );
                plan.operations.push(FileOperation::Replace {
                    old: (*old).clone(),
                    incoming: file.clone(),
                    observed: actual.identity.clone(),
                });
            }
            (Some(_), None) | (None, None) => plan.operations.push(FileOperation::Add {
                incoming: file.clone(),
            }),
            (None, Some(_)) => anyhow::bail!("新资源与未拥有的文件冲突"),
        }
    }
    for file in &receipt.files {
        let key = normalize_relative_path(&file.relative_path)?;
        if new.contains_key(&key) {
            continue;
        }
        if let Some(actual) = actual.get(&key) {
            if actual.is_reparse
                || actual.link_count != 1
                || actual.identity.size != file.size
                || actual.identity.sha256 != file.sha256
            {
                plan.retained_conflicts.push(file.relative_path.clone());
            } else {
                plan.operations.push(FileOperation::RemoveOwned {
                    old: file.clone(),
                    observed: actual.identity.clone(),
                });
            }
        }
    }
    Ok(plan)
}
pub fn preview_uninstall(
    receipt: &InstallReceipt,
    selection: DataSelection,
    verified_database: Option<(PathBuf, FileIdentity)>,
    configuration_files: Vec<PathBuf>,
) -> Result<UninstallPreview> {
    validate_receipt(receipt)?;
    ensure!(
        receipt.owner != InstallOwner::LegacyPortable,
        "便携版尚未建立可信安装记录"
    );
    ensure!(
        !selection.database || verified_database.is_some(),
        "数据库尚未验证"
    );
    let mut protected = ProtectedPaths {
        files: configuration_files.clone(),
        directories: vec![],
    };
    if let Some((database, identity)) = &verified_database {
        absolute_components(database)?;
        ensure!(
            database.file_name().and_then(|n| n.to_str()) == Some("studyapp.sqlite3")
                && valid_sha256(&identity.sha256),
            "数据库身份无效"
        );
        protected.directories.push(
            database
                .parent()
                .ok_or_else(|| anyhow::anyhow!("数据库目录无效"))?
                .to_owned(),
        );
    }
    check_protected(&receipt.install_root, &receipt.files, &protected)?;
    let (database_path, database_identity) = verified_database
        .map(|(p, i)| (Some(p), Some(i)))
        .unwrap_or((None, None));
    // Sidecars are captured individually by the adapter before execution. This
    // preview never infers journal/WAL ownership from a directory sweep.
    let database_files = if selection.database {
        database_path.iter().cloned().collect()
    } else {
        vec![]
    };
    Ok(UninstallPreview {
        preview_id: uuid::Uuid::new_v4().to_string(),
        install_id: receipt.install_id.clone(),
        database_path,
        database_identity,
        selection,
        application_files: receipt.files.clone(),
        configuration_files: if selection.configuration {
            configuration_files
        } else {
            vec![]
        },
        database_files,
    })
}
