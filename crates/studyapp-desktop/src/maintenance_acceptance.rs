//! Isolated Windows-runner acceptance. Compiled only with the ui-test feature.
//! Fixture arguments are never production deletion authority.
use super::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
#[path = "maintenance_acceptance_process.rs"]
mod process;

const NOW: i64 = 1_800_000_000;

struct Options {
    fixture_root: PathBuf,
    package: PathBuf,
    msi: PathBuf,
    evidence: PathBuf,
    staged: bool,
    suite: String,
    recovery_runtime: Option<PathBuf>,
}

pub(crate) fn run_if_requested(arguments: &[String]) -> Result<bool> {
    let requested = arguments.iter().any(|arg| {
        matches!(
            arg.as_str(),
            "--maintenance-execution-smoke" | "--maintenance-staged-uninstall-smoke"
        )
    });
    if !requested {
        return Ok(false);
    }
    ensure!(
        std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true")
            && std::env::var("RUNNER_OS").as_deref() == Ok("Windows"),
        "Native maintenance acceptance is restricted to the Windows CI runner"
    );
    let options = parse_options(arguments)?;
    let fixture_root = options.fixture_root.clone();
    let runner = PathBuf::from(std::env::var_os("RUNNER_TEMP").context("RUNNER_TEMP is required")?);
    let root_components = absolute_components(&fixture_root)?;
    let runner_components = absolute_components(&runner)?;
    ensure!(
        root_components.len() > runner_components.len()
            && root_components.starts_with(&runner_components),
        "Acceptance fixtures must be a child of RUNNER_TEMP"
    );
    let executable = std::env::current_exe()?;
    if !absolute_components(&executable)?.starts_with(&runner_components) {
        ensure!(
            options.suite == "msi-owner",
            "Copy the ui-test driver outside the checkout into RUNNER_TEMP"
        );
        let default_root =
            PathBuf::from(std::env::var_os("LOCALAPPDATA").context("LOCALAPPDATA required")?)
                .join("Programs/WW-StudyApp");
        let context = runtime::DiscoveryContext {
            default_library: fixture_root.join("library"),
            location_file: None,
            explicit_library: Some(fixture_root.join("library")),
        };
        let receipt = runtime::discover(&executable, &context)
            .receipt
            .context("Private MSI validation driver must have verified ownership")?;
        ensure!(
            matches!(receipt.owner, InstallOwner::Msi { .. })
                && receipt.version == "0.0.1"
                && same_absolute_path(&receipt.install_root, &default_root)?,
            "Only the freshly installed private MSI validation image may run outside RUNNER_TEMP"
        );
        msi::verify_registration(&receipt)?;
    }
    let evidence = options.evidence.clone();
    ensure!(
        same_absolute_path(&evidence, &fixture_root.join("maintenance-execution.json"))?,
        "Acceptance evidence must belong to its fresh fixture root"
    );
    ensure!(
        !fixture_root.try_exists()?,
        "Acceptance requires a fresh fixture root"
    );
    crate::maintenance_handoff::create_private_directory(&fixture_root)?;
    let mut records = Vec::new();
    let result = if options.staged {
        staged_uninstall(&options, &mut records)
    } else if options.suite == "msi-owner" {
        msi_owner_case(&options, &mut records)
    } else if options.suite == "msi-cleanup" {
        msi_cleanup_case(&options, &mut records)
    } else if options.suite.starts_with("recovery-prepare-") {
        recovery_boundary(&options, options.suite == "recovery-prepare-mixed")
    } else if options.suite == "recovery-ui" {
        let request = discover_recovery(&std::env::current_exe()?)?
            .context("Verified recovery request missing from actual snapshot entry")?;
        crate::maintenance_ui::run_recovery_acceptance(
            request,
            fixture_root.join("recovery-handoff.json"),
        )
    } else if options.suite == "health" {
        real_health_cases(&options, &mut records)
    } else if options.suite == "repair" {
        record(&mut records, "offline-repair-retry", || {
            crate::update_ui::run_repair_acceptance(&options.fixture_root, &options.package)
        })
    } else if options.suite.starts_with("cancel-") {
        let stage = match options.suite.as_str() {
            "cancel-selection" => ConfirmationStage::Selection,
            "cancel-database-review" => ConfirmationStage::DatabaseReview,
            "cancel-database-final" => ConfirmationStage::DatabaseFinal,
            _ => unreachable!(),
        };
        record(&mut records, &options.suite, || {
            cancel_case(&fixture_root, stage, &options.suite)
        })
    } else {
        run_matrix(&options, &mut records)
    };
    let msi_hash = runtime::observe_file(&options.msi, "package")?
        .identity
        .sha256;
    let report = json!({"schema":1,"native_windows_runner":true,"version":env!("CARGO_PKG_VERSION"),"source_sha":std::env::var("GITHUB_SHA").unwrap_or_default(),"msi_sha256":msi_hash,"scenarios":records,"complete":result.is_ok(),"error":result.as_ref().err().map(|error|format!("{error:#}"))});
    create_new_file(&evidence, &serde_json::to_vec_pretty(&report)?)?;
    result?;
    Ok(true)
}

fn parse_options(arguments: &[String]) -> Result<Options> {
    let mut values = BTreeMap::new();
    let mut staged = false;
    let mut modes = 0;
    let mut index = 0;
    while index < arguments.len() {
        let name = arguments[index].as_str();
        if matches!(
            name,
            "--maintenance-execution-smoke" | "--maintenance-staged-uninstall-smoke"
        ) {
            modes += 1;
            staged = name == "--maintenance-staged-uninstall-smoke";
            index += 1;
            continue;
        }
        ensure!(
            [
                "--fixture-root",
                "--package",
                "--msi",
                "--evidence",
                "--suite",
                "--data-dir",
                "--recovery-runtime"
            ]
            .contains(&name),
            "Unknown acceptance argument"
        );
        let value = arguments
            .get(index + 1)
            .context("Missing acceptance argument value")?;
        ensure!(
            values.insert(name, PathBuf::from(value)).is_none(),
            "Duplicate acceptance argument"
        );
        index += 2;
    }
    let suite = values
        .remove("--suite")
        .map(|path| {
            path.into_os_string()
                .into_string()
                .map_err(|_| anyhow::anyhow!("Invalid acceptance suite"))
        })
        .transpose()?
        .unwrap_or_else(|| "journal".into());
    ensure!(
        [
            "journal",
            "msi-owner",
            "msi-cleanup",
            "health",
            "repair",
            "cancel-selection",
            "cancel-database-review",
            "cancel-database-final",
            "recovery-prepare-missing",
            "recovery-prepare-mixed",
            "recovery-ui"
        ]
        .contains(&suite.as_str()),
        "Unknown acceptance suite"
    );
    let recovery_runtime = values.remove("--recovery-runtime");
    ensure!(
        options_require_runtime(&suite) == recovery_runtime.is_some(),
        "Recovery preparation requires exactly one source runtime directory"
    );
    ensure!(
        modes == 1 && values.len() == 5,
        "Incomplete acceptance arguments"
    );
    ensure!(
        same_absolute_path(
            values
                .get("--data-dir")
                .context("Fresh fallback data path required")?,
            &values
                .get("--fixture-root")
                .context("Fixture root required")?
                .join("fallback-data")
        )?,
        "Acceptance fallback startup data must belong to its fresh fixture"
    );
    values.remove("--data-dir");
    Ok(Options {
        fixture_root: values.remove("--fixture-root").unwrap(),
        package: values.remove("--package").unwrap(),
        msi: values.remove("--msi").unwrap(),
        evidence: values.remove("--evidence").unwrap(),
        staged,
        suite,
        recovery_runtime,
    })
}

fn options_require_runtime(suite: &str) -> bool {
    suite.starts_with("recovery-prepare-")
}

fn recovery_boundary(options: &Options, mixed: bool) -> Result<()> {
    let package_hash = runtime::observe_file(&options.package, "package")?
        .identity
        .sha256;
    let verified = archive::read_archive(&options.package, &package_hash)?;
    ensure!(
        verified.manifest.version == env!("CARGO_PKG_VERSION"),
        "Recovery package/build versions differ"
    );
    let root = directory(&options.fixture_root, "recovery-install")?;
    directory(&root, "support")?;
    let source_runtime = options
        .recovery_runtime
        .as_ref()
        .context("Old runtime pair required")?;
    let old_files = materialize_recovery_fixture(
        &verified,
        |resource| match resource.role {
            FileRole::Application | FileRole::Uninstaller => {
                Ok(std::fs::read(std::env::current_exe()?)?)
            }
            FileRole::Runtime => Ok(std::fs::read(source_runtime.join(&resource.relative_path))?),
            _ => Ok(verified.members[&resource.relative_path].clone()),
        },
        |name, bytes| create_new_file(&root.join(name), bytes),
    )?;
    let receipt = InstallReceipt {
        schema: 1,
        install_id: uuid::Uuid::new_v4().to_string(),
        owner: InstallOwner::Portable,
        install_root: root.clone(),
        root_identity: runtime::handle_identity(
            &crate::storage_backend::plain_handle(&root, true, false)?,
            true,
        )?
        .0,
        version: "0.0.1".into(),
        source_package_sha256: "a".repeat(64),
        files: old_files,
    };
    create_new_file(&root.join(RECEIPT_PATH), &serde_json::to_vec(&receipt)?)?;
    create_new_file(
        &root.join("unknown-user-note.md"),
        b"Unknown original must survive actual recovery",
    )?;
    let names = receipt
        .files
        .iter()
        .map(|file| file.relative_path.clone())
        .chain([
            RECEIPT_PATH.into(),
            "support/manifest.json".into(),
            "unknown-user-note.md".into(),
        ])
        .collect::<Vec<_>>();
    let before = hashes(&root, &names)?;
    let data = data_fixture(&options.fixture_root, "recovery-user-data")?;
    let data_root = options.fixture_root.join("recovery-user-data");
    let data_before = fixture_tree_hashes(&data_root)?;
    let prepared = prepare_portable_update(
        &options.package,
        &package_hash,
        &receipt,
        &ProtectedPaths {
            files: data
                .preserved
                .iter()
                .map(|(path, _)| path.clone())
                .collect(),
            directories: vec![data.context.default_library.clone()],
        },
        env!("CARGO_PKG_VERSION"),
    )?;
    let plan = load_plan(&prepared)?;
    let runtime_ops = plan
        .operations
        .iter()
        .enumerate()
        .filter(|(_, op)| {
            op.relative_path == "vcruntime140.dll" || op.relative_path == "vcruntime140_1.dll"
        })
        .collect::<Vec<_>>();
    ensure!(
        runtime_ops.len() == 2,
        "Recovery fixture must update both actual runtime DLLs"
    );
    if mixed {
        ensure!(
            runtime_ops
                .iter()
                .all(|(_, op)| op.original.as_ref().unwrap().sha256
                    != op.incoming.as_ref().unwrap().sha256),
            "Mixed runtime requires both genuine old DLL hashes to differ from incoming"
        );
    }
    let boundary_index = runtime_ops[0].0;
    let boundary_name = runtime_ops[0].1.relative_path.clone();
    let other_name = runtime_ops[1].1.relative_path.clone();
    let request = RecoveryRequest {
        prepared: prepared.clone(),
        source_receipt: receipt.clone(),
    };
    let snapshot = recovery_snapshot(&request)?;
    let expected_boundary = if mixed { "installed" } else { "backed_up" };
    let msi_hash = runtime::observe_file(&options.msi, "package")?
        .identity
        .sha256;
    let _status = apply_with_health(
        &prepared,
        |_| bail!("Recovery crash boundary was not reached"),
        |event, index| {
            if event == expected_boundary && index == boundary_index {
                ensure!(
                    fixture_tree_hashes(&data_root)? == data_before,
                    "Journal changed user fixture before crash"
                );
                let mut observed_runtime_hashes = BTreeMap::new();
                if mixed {
                    for (_, operation) in &runtime_ops {
                        observed_runtime_hashes.insert(
                            operation.relative_path.clone(),
                            runtime::observe_file(
                                &root.join(&operation.relative_path),
                                "mixed-runtime",
                            )?
                            .identity
                            .sha256,
                        );
                    }
                    let complete_old = runtime_ops.iter().all(|(_, operation)| {
                        observed_runtime_hashes[&operation.relative_path]
                            == operation.original.as_ref().unwrap().sha256
                    });
                    let complete_new = runtime_ops.iter().all(|(_, operation)| {
                        observed_runtime_hashes[&operation.relative_path]
                            == operation.incoming.as_ref().unwrap().sha256
                    });
                    ensure!(
                        !complete_old && !complete_new,
                        "Observed runtime pair is entirely old or entirely new"
                    );
                    ensure!(
                        runtime::observe_file(&root.join(&boundary_name), "incoming-runtime")?
                            .identity
                            .sha256
                            == plan.operations[index].incoming.as_ref().unwrap().sha256
                            && runtime::observe_file(&root.join(&other_name), "old-runtime")?
                                .identity
                                .sha256
                                == before[&other_name],
                        "Root is not at the actual mixed runtime boundary"
                    );
                } else {
                    ensure!(
                        !root.join(&boundary_name).try_exists()?,
                        "Root required DLL was not actually missing"
                    );
                }
                ensure!(
                    recovery_snapshot(&request)? == snapshot,
                    "Verified old snapshot changed before crash"
                );
                let detail = json!({"scenario":if mixed {"recovery-mixed-runtime"} else {"recovery-missing-runtime"},"transaction_id":prepared.transaction_id,"snapshot":snapshot,"install_root":root,"old_hashes":before,"data_root":data_root,"data_hashes":data_before,"boundary":event,"boundary_index":index,"runtime_name":boundary_name,"other_runtime_name":other_name,"actual_root_runtime_missing":!mixed,"actual_mixed_generation":mixed,"observed_runtime_hashes":observed_runtime_hashes,"runtime_operations":runtime_ops.iter().map(|(_,op)|json!({"relative_path":op.relative_path,"old":op.original,"incoming":op.incoming})).collect::<Vec<_>>()});
                create_new_file(
                    &options.fixture_root.join("recovery-boundary.json"),
                    &serde_json::to_vec_pretty(&detail)?,
                )?;
                let report = json!({"schema":1,"native_windows_runner":true,"version":env!("CARGO_PKG_VERSION"),"source_sha":std::env::var("GITHUB_SHA").unwrap_or_default(),"msi_sha256":msi_hash,"scenarios":[],"complete":false,"intentional_crash_exit":86,"boundary":detail});
                create_new_file(&options.evidence, &serde_json::to_vec_pretty(&report)?)?;
                // Genuine abrupt fixture-process death. No unwinding, automatic
                // rollback, manual DLL copy or accepting health callback occurs.
                std::process::exit(86);
            }
            Ok(())
        },
    )?;
    bail!("Actual journal crash boundary was not reached")
}

fn materialize_recovery_fixture(
    verified: &archive::VerifiedArchive,
    mut read_resource: impl FnMut(&OwnedFile) -> Result<Vec<u8>>,
    mut create_resource: impl FnMut(&str, &[u8]) -> Result<FileIdentity>,
) -> Result<Vec<OwnedFile>> {
    let mut old_files = vec![];
    // read_archive includes the incoming manifest's identity. Build the old
    // manifest once from old resources, then record its actual created identity.
    for resource in verified
        .manifest
        .files
        .iter()
        .filter(|file| file.role != FileRole::Manifest)
    {
        let bytes = read_resource(resource)?;
        let identity = create_resource(&resource.relative_path, &bytes)?;
        old_files.push(OwnedFile {
            relative_path: resource.relative_path.clone(),
            role: resource.role,
            size: identity.size,
            sha256: identity.sha256,
        });
    }
    let old_manifest = PackageManifest {
        schema: 1,
        product: PRODUCT.into(),
        architecture: ARCHITECTURE.into(),
        version: "0.0.1".into(),
        files: old_files.clone(),
    };
    let manifest_identity = create_resource(
        "support/manifest.json",
        &serde_json::to_vec_pretty(&old_manifest)?,
    )?;
    old_files.push(OwnedFile {
        relative_path: "support/manifest.json".into(),
        role: FileRole::Manifest,
        size: manifest_identity.size,
        sha256: manifest_identity.sha256,
    });
    Ok(old_files)
}

fn record(
    records: &mut Vec<Value>,
    name: &str,
    body: impl FnOnce() -> Result<Value>,
) -> Result<()> {
    let details = body()?;
    records.push(json!({"scenario":name,"passed":true,"executor":"real","details":details}));
    Ok(())
}

fn directory(parent: &Path, name: &str) -> Result<PathBuf> {
    let path = parent.join(name);
    crate::maintenance_handoff::create_private_directory(&path)?;
    Ok(path)
}

fn hashes(root: &Path, names: &[String]) -> Result<BTreeMap<String, String>> {
    names
        .iter()
        .map(|name| {
            Ok((
                name.clone(),
                runtime::observe_file(&root.join(name), name)?
                    .identity
                    .sha256,
            ))
        })
        .collect()
}

fn tiny_receipt(root: &Path) -> Result<InstallReceipt> {
    directory(root, "support")?;
    let mut files = Vec::new();
    for (name, bytes) in [
        ("runtime.dll", b"old-runtime".as_slice()),
        ("retired.dll", b"old-retired".as_slice()),
    ] {
        let identity = create_new_file(&root.join(name), bytes)?;
        files.push(OwnedFile {
            relative_path: name.into(),
            role: FileRole::Runtime,
            size: identity.size,
            sha256: identity.sha256,
        });
    }
    let root_identity = runtime::handle_identity(
        &crate::storage_backend::plain_handle(root, true, false)?,
        true,
    )?
    .0;
    let receipt = InstallReceipt {
        schema: 1,
        install_id: uuid::Uuid::new_v4().to_string(),
        owner: InstallOwner::Portable,
        install_root: root.into(),
        root_identity,
        version: "0.0.1".into(),
        source_package_sha256: "a".repeat(64),
        files,
    };
    create_new_file(&root.join(RECEIPT_PATH), &serde_json::to_vec(&receipt)?)?;
    Ok(receipt)
}

fn journal_fixture(
    parent: &Path,
    name: &str,
) -> Result<(PreparedMaintenance, Vec<String>, BTreeMap<String, String>)> {
    let root = directory(parent, name)?;
    let receipt = tiny_receipt(&root)?;
    create_new_file(&root.join("unknown-note.md"), b"original-note")?;
    let names = vec![
        "runtime.dll".into(),
        "retired.dll".into(),
        RECEIPT_PATH.into(),
        "unknown-note.md".into(),
    ];
    let old = hashes(&root, &names)?;
    let (id, stage) = new_transaction(&root)?;
    let mut next = receipt.clone();
    next.version = env!("CARGO_PKG_VERSION").into();
    next.files.clear();
    let mut operations = Vec::new();
    for (index, (relative, bytes)) in [
        ("runtime.dll", Some(b"new-runtime".as_slice())),
        ("retired.dll", None),
        ("added.dll", Some(b"new-added".as_slice())),
    ]
    .into_iter()
    .enumerate()
    {
        let original = if root.join(relative).try_exists()? {
            Some(runtime::observe_file(&root.join(relative), relative)?.identity)
        } else {
            None
        };
        let staged = stage.join(format!("new-{index}"));
        let incoming = bytes
            .map(|bytes| create_new_file(&staged, bytes))
            .transpose()?;
        if let Some(identity) = &incoming {
            next.files.push(OwnedFile {
                relative_path: relative.into(),
                role: FileRole::Runtime,
                size: identity.size,
                sha256: identity.sha256.clone(),
            });
        }
        operations.push(JournalOperation {
            relative_path: relative.into(),
            original,
            incoming,
            staged,
            backup: stage.join(format!("old-{index}")),
        });
    }
    add_receipt_operation(&stage, &receipt, &next, &mut operations)?;
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
    )?;
    Ok((prepared, names, old))
}

fn journal_matrix(parent: &Path) -> Result<Value> {
    let mut observed = Vec::new();
    for index in 0..4 {
        for boundary in ["before", "backed_up", "installed"] {
            let (prepared, names, old) =
                journal_fixture(parent, &format!("fault-{boundary}-{index}"))?;
            let status = apply_with_health(
                &prepared,
                |_| Ok(()),
                |event, current| {
                    if event == boundary && current == index {
                        bail!("Controlled journal boundary failure");
                    }
                    Ok(())
                },
            )?;
            ensure!(
                status.state == MaintenanceState::RolledBack,
                "Journal did not roll back {boundary}/{index}"
            );
            ensure!(
                hashes(&prepared.install_root, &names)? == old
                    && !prepared.install_root.join("added.dll").try_exists()?,
                "Rollback changed previous payload or unknown notes"
            );
            observed.push(json!({"boundary":boundary,"index":index,"status":status}));
        }
    }
    let (prepared, names, old) = journal_fixture(parent, "fault-health")?;
    let status = apply_with_health(
        &prepared,
        |_| bail!("Controlled missing health acknowledgement"),
        |_, _| Ok(()),
    )?;
    ensure!(
        status.state == MaintenanceState::RolledBack
            && hashes(&prepared.install_root, &names)? == old,
        "Missing health acknowledgement did not restore old files"
    );
    observed.push(json!({"boundary":"health","status":status}));
    Ok(json!({"boundaries":observed,"injected_health_is_not_restart_evidence":true}))
}

struct DataFixture {
    receipt: InstallReceipt,
    context: runtime::DiscoveryContext,
    database: PathBuf,
    preserved: Vec<(PathBuf, String)>,
    configuration: Vec<(PathBuf, String)>,
    history: Value,
    settings: studyapp_core::Settings,
}

fn history(snapshot: &studyapp_core::Snapshot) -> Value {
    json!({"notes":snapshot.notes,"versions":snapshot.versions,"sessions":snapshot.sessions,"reviews":snapshot.reviews,"review_events":snapshot.review_events})
}

fn data_fixture(parent: &Path, name: &str) -> Result<DataFixture> {
    let root = directory(parent, name)?;
    let install = directory(&root, "install")?;
    let receipt = tiny_receipt(&install)?;
    let library = directory(&root, "library")?;
    let inside = library.join("tracked-original.md");
    let outside = root.join("outside-original.md");
    create_new_file(&inside, b"# Original inside library\nKeep this source.")?;
    create_new_file(&outside, b"# Original outside library\nKeep this source.")?;
    let database = library.join("studyapp.sqlite3");
    let mut engine = studyapp_core::Engine::open(&database, NOW)?;
    engine.apply(
        studyapp_core::Command::Import(vec![inside.clone(), outside.clone()]),
        NOW,
    )?;
    let mut settings = engine.snapshot().settings;
    settings.interval_minutes = 41;
    engine.apply(studyapp_core::Command::SaveSettings(settings.clone()), NOW)?;
    let (preferences, sync) = crate::preferences::PreferencesStore::open(&database, &mut engine)?;
    let original_history = history(&engine.snapshot());
    let inside_canonical = std::fs::canonicalize(&inside)?;
    let outside_canonical = std::fs::canonicalize(&outside)?;
    ensure!(
        engine
            .snapshot()
            .notes
            .iter()
            .any(|note| Path::new(&note.path) == inside_canonical)
            && engine
                .snapshot()
                .notes
                .iter()
                .any(|note| Path::new(&note.path) == outside_canonical),
        "Fixture originals were not tracked by the real engine"
    );
    drop(preferences);
    drop(sync);
    drop(engine);
    create_new_file(&library.join("studyapp.lock"), b"fixture-lock")?;
    let unknown = install.join("unknown-neighbour.md");
    create_new_file(&unknown, b"unknown adjacent original")?;
    let image = library.join("unknown-image.png");
    create_new_file(&image, b"user-image")?;
    let backup = library.join("retained-backup.sqlite3");
    create_new_file(&backup, b"user-backup")?;
    let unknown_settings = library.join("settings/user-owned.txt");
    create_new_file(&unknown_settings, b"user-owned settings neighbour")?;
    let context = runtime::DiscoveryContext {
        default_library: library.clone(),
        location_file: None,
        explicit_library: Some(library),
    };
    let discovery = runtime::discover_library(&context);
    ensure!(
        discovery.issues.is_empty(),
        "Valid fixture library/config discovery failed"
    );
    let capture = |paths: Vec<PathBuf>| -> Result<Vec<(PathBuf, String)>> {
        paths
            .into_iter()
            .map(|path| {
                Ok((
                    path.clone(),
                    runtime::observe_file(&path, "fixture")?.identity.sha256,
                ))
            })
            .collect()
    };
    let preserved = capture(vec![
        inside,
        outside,
        unknown,
        image,
        backup,
        unknown_settings,
    ])?;
    let configuration = capture(discovery.configuration_files)?;
    Ok(DataFixture {
        receipt,
        context,
        database,
        preserved,
        configuration,
        history: original_history,
        settings,
    })
}

fn unchanged(files: &[(PathBuf, String)]) -> Result<()> {
    for (path, expected) in files {
        ensure!(
            runtime::observe_file(path, "fixture")?.identity.sha256 == *expected,
            "User fixture file was changed: {}",
            path.display()
        );
    }
    Ok(())
}

fn confirmation(fixture: &DataFixture, selection: DataSelection) -> Result<UninstallConfirmation> {
    let discovery = runtime::discover_library(&fixture.context);
    Ok(UninstallConfirmation::new(preview_uninstall(
        &fixture.receipt,
        selection,
        discovery.verified_database,
        discovery.configuration_files,
    )?))
}

fn confirmed(fixture: &DataFixture, selection: DataSelection) -> Result<ConfirmedUninstall> {
    let mut confirmation = confirmation(fixture, selection)?;
    confirmation.confirm_selection();
    if selection.database {
        confirmation.confirm_database_review()?;
        confirmation.confirm_database_final("删除数据库")?;
    }
    confirmation.finish()
}

fn uninstall_data(parent: &Path, name: &str, selection: DataSelection) -> Result<Value> {
    let fixture = data_fixture(parent, name)?;
    let original_database = runtime::observe_file(&fixture.database, "database")?
        .identity
        .sha256;
    let intent = prepare_uninstall(
        confirmed(&fixture, selection)?,
        &fixture.receipt,
        &fixture.context,
    )?;
    let status = apply_authenticated_uninstall(intent)?;
    ensure!(
        status.state == MaintenanceState::Succeeded,
        "Actual uninstall did not succeed: {}",
        status.code
    );
    for file in &fixture.receipt.files {
        ensure!(
            !fixture
                .receipt
                .install_root
                .join(&file.relative_path)
                .try_exists()?,
            "Owned application file remains"
        );
    }
    unchanged(&fixture.preserved)?;
    if selection.database {
        ensure!(!fixture.database.try_exists()?, "Selected database remains");
    } else if !selection.configuration {
        ensure!(
            runtime::observe_file(&fixture.database, "database")?
                .identity
                .sha256
                == original_database,
            "Default uninstall changed database bytes"
        );
    }
    if selection.configuration {
        for (path, _) in &fixture.configuration {
            ensure!(!path.try_exists()?, "Selected known configuration remains");
        }
    } else {
        unchanged(&fixture.configuration)?;
    }
    // Reopen the same startup data adapters after removal. This is real typed
    // settings migration/reset validation; no normal UI or Worker is initialized.
    let mut reopened = studyapp_core::Engine::open(&fixture.database, NOW)?;
    let (preferences, sync) =
        crate::preferences::PreferencesStore::open(&fixture.database, &mut reopened)?;
    ensure!(
        reopened.snapshot().settings
            == if selection.configuration {
                studyapp_core::Settings::default()
            } else {
                fixture.settings.clone()
            },
        "Preferences were not correctly retained/reset after reinstall"
    );
    if !selection.database {
        ensure!(
            history(&reopened.snapshot()) == fixture.history,
            "Configuration reset changed note/version/learning history"
        );
    }
    drop(preferences);
    drop(sync);
    drop(reopened);
    unchanged(&fixture.preserved)?;
    Ok(
        json!({"status":status,"selection":{"configuration":selection.configuration,"database":selection.database},"database_confirmations":if selection.database {2}else{0},"settings_reopened":true,"originals_images_backups_unknown_retained":true}),
    )
}

fn fixture_tree_hashes(root: &Path) -> Result<BTreeMap<PathBuf, String>> {
    let mut pending = vec![root.to_owned()];
    let mut files = BTreeMap::new();
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory)? {
            let path = entry?.path();
            let metadata = std::fs::symlink_metadata(&path)?;
            ensure!(
                !metadata.file_type().is_symlink(),
                "Unexpected fixture link"
            );
            if metadata.is_dir() {
                pending.push(path);
            } else {
                files.insert(
                    path.strip_prefix(root)?.to_owned(),
                    runtime::observe_file(&path, "complete-fixture")?
                        .identity
                        .sha256,
                );
            }
        }
    }
    Ok(files)
}

fn cancel_case(parent: &Path, stage: ConfirmationStage, name: &str) -> Result<Value> {
    let fixture = data_fixture(parent, name)?;
    let complete_before = fixture_tree_hashes(&parent.join(name))?;
    let old = runtime::observe_file(&fixture.database, "database")?
        .identity
        .sha256;
    let names = fixture
        .receipt
        .files
        .iter()
        .map(|file| file.relative_path.clone())
        .chain(std::iter::once(RECEIPT_PATH.into()))
        .collect::<Vec<_>>();
    let before = hashes(&fixture.receipt.install_root, &names)?;
    let screenshot = parent.join(format!("{name}.png"));
    let calls = crate::maintenance_ui::run_cancel_acceptance(
        runtime::InstallDiscovery {
            receipt: Some(fixture.receipt.clone()),
            library: runtime::discover_library(&fixture.context),
            issues: vec![],
        },
        fixture.context.clone(),
        stage,
        screenshot.clone(),
    )?;
    ensure!(
        calls == 0
            && screenshot.is_file()
            && hashes(&fixture.receipt.install_root, &names)? == before,
        "Cancel changed complete application snapshot or invoked handoff"
    );
    ensure!(
        fixture_tree_hashes(&parent.join(name))? == complete_before,
        "Cancellation changed complete fixture tree, database sidecars, configuration, or control files"
    );
    unchanged(&fixture.preserved)?;
    unchanged(&fixture.configuration)?;
    ensure!(
        runtime::observe_file(&fixture.database, "database")?
            .identity
            .sha256
            == old,
        "Cancellation changed database"
    );
    for file in &fixture.receipt.files {
        ensure!(
            runtime::observe_file(
                &fixture.receipt.install_root.join(&file.relative_path),
                "owned"
            )?
            .identity
            .sha256
                == file.sha256,
            "Cancellation changed application"
        );
    }
    Ok(
        json!({"stage":format!("{stage:?}"),"executor_calls":calls,"captured_files_unchanged":true,"actual_cancel_event":true,"production_handoff_bound":true,"screenshot":screenshot}),
    )
}

fn damaged_database(parent: &Path, missing: bool) -> Result<Value> {
    let fixture = data_fixture(
        parent,
        if missing {
            "uninstall-missing-db"
        } else {
            "uninstall-corrupt-db"
        },
    )?;
    if missing {
        let identity = runtime::observe_file(&fixture.database, "database")?.identity;
        file_ops::delete_exact(&fixture.database, &identity)?;
    } else {
        std::fs::write(&fixture.database, b"corrupt database fixture")?;
    }
    let old = if missing {
        None
    } else {
        Some(
            runtime::observe_file(&fixture.database, "database")?
                .identity
                .sha256,
        )
    };
    let intent = prepare_uninstall(
        confirmed(&fixture, DataSelection::default())?,
        &fixture.receipt,
        &fixture.context,
    )?;
    let status = apply_authenticated_uninstall(intent)?;
    ensure!(
        status.state == MaintenanceState::Succeeded,
        "Default application removal depends on healthy database"
    );
    ensure!(
        if let Some(hash) = old {
            runtime::observe_file(&fixture.database, "database")?
                .identity
                .sha256
                == hash
        } else {
            !fixture.database.try_exists()?
        },
        "Default removal changed damaged database state"
    );
    unchanged(&fixture.preserved)?;
    unchanged(&fixture.configuration)?;
    Ok(json!({"status":status,"database_missing":missing,"data_retained":true}))
}

fn shared_lock(parent: &Path) -> Result<Value> {
    let fixture = data_fixture(parent, "shared-lock")?;
    let old = runtime::observe_file(&fixture.database, "database")?
        .identity
        .sha256;
    let intent = prepare_uninstall(
        confirmed(
            &fixture,
            DataSelection {
                configuration: true,
                database: true,
            },
        )?,
        &fixture.receipt,
        &fixture.context,
    )?;
    let lock = runtime::acquire_existing_library_lock(fixture.database.parent().unwrap())?;
    let status = apply_authenticated_uninstall(intent)?;
    ensure!(
        status.state == MaintenanceState::Failed
            && status.code == "application_removed_data_retained",
        "Shared library lock was not reported accurately"
    );
    ensure!(
        runtime::observe_file(&fixture.database, "database")?
            .identity
            .sha256
            == old,
        "Locked database changed"
    );
    unchanged(&fixture.configuration)?;
    unchanged(&fixture.preserved)?;
    drop(lock);
    Ok(json!({"status":status,"exclusive_library_lock_exercised":true}))
}

fn legacy_fixture(
    parent: &Path,
    name: &str,
    verified: &archive::VerifiedArchive,
) -> Result<PathBuf> {
    let root = directory(parent, name)?;
    for resource in &verified.manifest.files {
        if matches!(resource.role, FileRole::Application | FileRole::Runtime) {
            create_new_file(
                &root.join(&resource.relative_path),
                &verified.members[&resource.relative_path],
            )?;
        }
    }
    create_new_file(
        &root.join("legacy-unknown-original.md"),
        b"never historically sweep",
    )?;
    Ok(root)
}

fn repair_case(
    options: &Options,
    verified: &archive::VerifiedArchive,
    mode: &str,
    hash: &str,
) -> Result<Value> {
    let root = legacy_fixture(&options.fixture_root, mode, verified)?;
    let unknown = root.join("legacy-unknown-original.md");
    let original = runtime::observe_file(&unknown, "unknown")?.identity.sha256;
    if mode == "unknown-uninstaller-conflict" {
        create_new_file(&root.join("Uninstall.exe"), b"unknown executable fixture")?;
        ensure!(
            reconcile_current_package(
                &options.package,
                hash,
                &root.join("WW-StudyApp.exe"),
                &ProtectedPaths::default()
            )
            .is_err(),
            "Unknown uninstaller was overwritten"
        );
        ensure!(
            std::fs::read(root.join("Uninstall.exe"))? == b"unknown executable fixture"
                && !root.join(RECEIPT_PATH).try_exists()?,
            "Conflict repair changed unknown ownership"
        );
    } else {
        let receipt = reconcile_current_package(
            &options.package,
            hash,
            &root.join("WW-StudyApp.exe"),
            &ProtectedPaths::default(),
        )?;
        ensure!(
            receipt.owner == InstallOwner::Portable && receipt.files == verified.manifest.files,
            "Repair did not create exact verified ownership"
        );
        for file in &receipt.files {
            ensure!(
                runtime::observe_file(&root.join(&file.relative_path), "repaired")?
                    .identity
                    .sha256
                    == file.sha256,
                "Repaired component differs"
            );
        }
        ensure!(
            receipt
                .files
                .iter()
                .all(|file| file.relative_path != "legacy-unknown-original.md"),
            "Repair adopted unknown history"
        );
    }
    ensure!(
        runtime::observe_file(&unknown, "unknown")?.identity.sha256 == original,
        "Repair swept historical unknown content"
    );
    Ok(json!({"mode":mode,"verified_current_tag_package":true,"unknown_history_retained":true}))
}

fn modified_files(
    options: &Options,
    verified: &archive::VerifiedArchive,
    hash: &str,
) -> Result<Value> {
    let root = directory(&options.fixture_root, "modified-obsolete")?;
    let baseline = upgrade_fixture_payload(verified)?;
    let receipt = materialize_portable(&root, &baseline, hash, &baseline.manifest.version)?;
    // A changed obsolete notice is safe to retain. A changed runtime cannot
    // become a trusted recovery snapshot; exercise that refusal separately.
    std::fs::write(
        root.join("support/README.md"),
        b"externally modified obsolete file",
    )?;
    create_new_file(&root.join("unknown-source.md"), b"unknown original")?;
    let prepared = prepare_portable_update(
        &options.package,
        hash,
        &receipt,
        &ProtectedPaths::default(),
        env!("CARGO_PKG_VERSION"),
    )?;
    let status = apply_with_health(&prepared, |_| Ok(()), |_, _| Ok(()))?;
    ensure!(
        status.state == MaintenanceState::Succeeded
            && status
                .retained_paths
                .contains(&root.join("support/README.md")),
        "Modified obsolete resource was not retained/reported"
    );
    ensure!(
        std::fs::read(root.join("support/README.md"))? == b"externally modified obsolete file"
            && std::fs::read(root.join("unknown-source.md"))? == b"unknown original",
        "Unknown/modified original was changed"
    );
    let runtime_root = directory(&options.fixture_root, "modified-obsolete-runtime")?;
    let runtime_receipt =
        materialize_portable(&runtime_root, &baseline, hash, &baseline.manifest.version)?;
    std::fs::write(
        runtime_root.join("retired.dll"),
        b"externally modified obsolete runtime",
    )?;
    create_new_file(&runtime_root.join("unknown-source.md"), b"unknown original")?;
    let names = runtime_receipt
        .files
        .iter()
        .map(|file| file.relative_path.clone())
        .chain([RECEIPT_PATH.into(), "unknown-source.md".into()])
        .collect::<Vec<_>>();
    let before = hashes(&runtime_root, &names)?;
    let runtime_error = match prepare_portable_update(
        &options.package,
        hash,
        &runtime_receipt,
        &ProtectedPaths::default(),
        env!("CARGO_PKG_VERSION"),
    ) {
        Ok(_) => bail!("Modified runtime was accepted as a recovery source"),
        Err(error) => format!("{error:#}"),
    };
    ensure!(
        runtime_error.contains("恢复快照来源已改变") && hashes(&runtime_root, &names)? == before,
        "Modified runtime refusal changed owned/unknown files or failed for another reason"
    );
    let fixture = data_fixture(&options.fixture_root, "modified-uninstall")?;
    std::fs::write(
        fixture.receipt.install_root.join("runtime.dll"),
        b"externally modified current runtime",
    )?;
    let removal = match prepare_uninstall(
        confirmed(&fixture, DataSelection::default())?,
        &fixture.receipt,
        &fixture.context,
    ) {
        Ok(intent) => {
            let status = apply_authenticated_uninstall(intent)?;
            ensure!(
                status.state == MaintenanceState::Failed,
                "Modified application was removed"
            );
            json!({"status":status})
        }
        Err(error) => json!({"preflight_rejected":true,"reason":format!("{error:#}")}),
    };
    ensure!(
        fixture
            .receipt
            .install_root
            .join("retired.dll")
            .try_exists()?
            && std::fs::read(fixture.receipt.install_root.join("runtime.dll"))?
                == b"externally modified current runtime",
        "Modified application did not block preflight deletion"
    );
    unchanged(&fixture.preserved)?;
    unchanged(&fixture.configuration)?;
    Ok(
        json!({"update":status,"uninstall":removal,"modified_runtime":{"preparation_rejected":true,"reason":runtime_error,"owned_and_unknown_hashes_unchanged":true},"baseline_payload":"verified-current-package-with-synthetic-0.0.1-metadata","injected_health_is_not_restart_evidence":true}),
    )
}

fn run_matrix(options: &Options, records: &mut Vec<Value>) -> Result<()> {
    let hash = runtime::observe_file(&options.package, "package")?
        .identity
        .sha256;
    let verified = archive::read_archive(&options.package, &hash)?;
    ensure!(
        verified.manifest.version == env!("CARGO_PKG_VERSION"),
        "Acceptance package/build version differs"
    );
    msi::verify_package(&options.msi, env!("CARGO_PKG_VERSION"))?;
    record(records, "failed-transaction-rollback", || {
        journal_matrix(&options.fixture_root)
    })?;
    for (name, selection) in [
        ("uninstall-default-retain", DataSelection::default()),
        (
            "uninstall-config-only",
            DataSelection {
                configuration: true,
                database: false,
            },
        ),
        (
            "uninstall-db-only",
            DataSelection {
                configuration: false,
                database: true,
            },
        ),
        (
            "uninstall-both",
            DataSelection {
                configuration: true,
                database: true,
            },
        ),
    ] {
        record(records, name, || {
            uninstall_data(&options.fixture_root, name, selection)
        })?;
    }
    record(records, "uninstall-corrupt-db", || {
        damaged_database(&options.fixture_root, false)
    })?;
    record(records, "uninstall-missing-db", || {
        damaged_database(&options.fixture_root, true)
    })?;
    record(records, "shared-library-lock", || {
        shared_lock(&options.fixture_root)
    })?;
    record(records, "modified-and-unknown-adjacent", || {
        modified_files(options, &verified, &hash)
    })?;
    record(records, "tracked-original-inside-and-outside", || {
        // A separate real engine/import/uninstall run proves the tracked paths,
        // independent of the earlier selection matrix records.
        uninstall_data(
            &options.fixture_root,
            "tracked-originals",
            DataSelection {
                configuration: true,
                database: true,
            },
        )
    })?;
    for mode in ["same-version-repair", "unknown-uninstaller-conflict"] {
        record(records, mode, || {
            repair_case(options, &verified, mode, &hash)
        })?;
    }
    let legacy = legacy_helper_cases(options, &verified, &hash)?;
    record(
        records,
        "legacy-schema1-accepts-legacy-rejects-modern",
        || {
            ensure!(
                legacy["legacy_members"].as_u64() == Some(7)
                    && legacy["modern_rejection"].as_str() == Some("unexpected_executable"),
                "Actual schema1 archive validation did not match policy"
            );
            Ok(legacy.clone())
        },
    )?;
    record(records, "legacy-skip-release", || {
        ensure!(
            legacy["reconciled_after_actual_legacy_helper"].as_bool() == Some(true),
            "Legacy skip-release did not reach verified modern ownership"
        );
        Ok(legacy)
    })?;
    Ok(())
}

fn parse_legacy_acceptance_report(bytes: &[u8]) -> Result<Value> {
    // Windows PowerShell 5.1 emits one UTF-8 BOM for Set-Content -Encoding utf8.
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    Ok(serde_json::from_slice(bytes)?)
}

fn legacy_helper_cases(
    options: &Options,
    verified: &archive::VerifiedArchive,
    hash: &str,
) -> Result<Value> {
    use std::os::windows::{ffi::OsStringExt, process::CommandExt};
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetSystemDirectoryW(buffer: *mut u16, size: u32) -> u32;
    }
    let helper = options.fixture_root.join("actual-schema1-helper.ps1");
    create_new_file(&helper, include_str!("update_helper.ps1").as_bytes())?;
    let driver = options.fixture_root.join("legacy-native-driver.ps1");
    create_new_file(
        &driver,
        include_str!("../../../scripts/maintenance-legacy-native.ps1").as_bytes(),
    )?;
    let legacy = options
        .package
        .parent()
        .context("Package directory missing")?
        .join(format!(
            "WW-StudyApp-{}-windows-x86_64.zip",
            env!("CARGO_PKG_VERSION")
        ));
    let evidence = options.fixture_root.join("actual-legacy-helper");
    let mut buffer = [0u16; 32768];
    let length = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) };
    ensure!(
        length > 0 && length < buffer.len() as u32,
        "Cannot identify system PowerShell directory"
    );
    let powershell = PathBuf::from(std::ffi::OsString::from_wide(&buffer[..length as usize]))
        .join("WindowsPowerShell/v1.0/powershell.exe");
    let output = std::process::Command::new(powershell)
        // Rust inherits PowerShell 7's module path without its direct-child
        // compatibility filtering. Let Windows PowerShell rebuild its own path.
        .env_remove("PSModulePath")
        .args(["-NoProfile", "-NonInteractive", "-File"])
        .arg(&driver)
        .arg("-HelperPath")
        .arg(&helper)
        .arg("-LegacyPackage")
        .arg(&legacy)
        .arg("-ModernPackage")
        .arg(&options.package)
        .arg("-Version")
        .arg(env!("CARGO_PKG_VERSION"))
        .arg("-OutputDirectory")
        .arg(&evidence)
        .creation_flags(0x0800_0000)
        .output()?;
    create_new_file(
        &options.fixture_root.join("legacy-helper.stdout.log"),
        &output.stdout,
    )?;
    create_new_file(
        &options.fixture_root.join("legacy-helper.stderr.log"),
        &output.stderr,
    )?;
    ensure!(
        output.status.success(),
        "Actual schema1 helper acceptance failed"
    );
    let mut result =
        parse_legacy_acceptance_report(&std::fs::read(evidence.join("legacy-acceptance.json"))?)?;
    let install = PathBuf::from(
        result["install_root"]
            .as_str()
            .context("Legacy install root absent")?,
    );
    ensure!(
        absolute_components(&install)?.starts_with(&absolute_components(&evidence)?)
            && result["installed_main_sha256"].as_str()
                == Some(
                    verified
                        .manifest
                        .files
                        .iter()
                        .find(|file| file.role == FileRole::Application)
                        .context("Missing main resource")?
                        .sha256
                        .as_str()
                ),
        "Actual legacy replacement differs from current package"
    );
    let receipt = reconcile_current_package(
        &options.package,
        hash,
        &install.join("WW-StudyApp.exe"),
        &ProtectedPaths::default(),
    )?;
    ensure!(
        receipt.owner == InstallOwner::Portable && receipt.files == verified.manifest.files,
        "Skip-release reconciliation did not establish exact verified ownership"
    );
    let original = PathBuf::from(
        result["original_path"]
            .as_str()
            .context("Legacy original fixture missing")?,
    );
    ensure!(
        runtime::observe_file(&original, "original")?
            .identity
            .sha256
            == result["original_sha256"]
                .as_str()
                .context("Legacy original digest missing")?,
        "Legacy update or repair changed unknown original"
    );
    result["reconciled_after_actual_legacy_helper"] = json!(true);
    Ok(result)
}

fn materialize_portable(
    root: &Path,
    verified: &archive::VerifiedArchive,
    hash: &str,
    version: &str,
) -> Result<InstallReceipt> {
    directory(root, "support")?;
    for (name, bytes) in &verified.members {
        create_new_file(&root.join(name), bytes)?;
    }
    let receipt = InstallReceipt {
        schema: 1,
        install_id: uuid::Uuid::new_v4().to_string(),
        owner: InstallOwner::Portable,
        install_root: root.into(),
        root_identity: runtime::handle_identity(
            &crate::storage_backend::plain_handle(root, true, false)?,
            true,
        )?
        .0,
        version: version.into(),
        source_package_sha256: hash.into(),
        files: verified.manifest.files.clone(),
    };
    create_new_file(&root.join(RECEIPT_PATH), &serde_json::to_vec(&receipt)?)?;
    Ok(receipt)
}

fn upgrade_fixture_payload(
    verified: &archive::VerifiedArchive,
) -> Result<archive::VerifiedArchive> {
    // The executable/runtime bytes come from the verified current package. Only
    // this isolated fixture's baseline version and obsolete resources are
    // synthetic; this is not evidence of a historical release or old restart.
    let mut fixture = archive::VerifiedArchive {
        manifest: verified.manifest.clone(),
        members: verified.members.clone(),
    };
    for role in [FileRole::Application, FileRole::Uninstaller] {
        ensure!(
            fixture.manifest.files.iter().any(|file| file.role == role),
            "Upgrade fixture requires complete verified recovery entries"
        );
    }
    let runtime = fixture
        .members
        .get("vcruntime140.dll")
        .context("Upgrade fixture requires the genuine packaged runtime")?
        .clone();
    fixture.manifest.version = "0.0.1".into();
    fixture
        .manifest
        .files
        .retain(|file| file.role != FileRole::Manifest);
    for (name, role, bytes) in [
        ("retired.dll", FileRole::Runtime, runtime),
        (
            "support/README.md",
            FileRole::Notice,
            b"Obsolete acceptance fixture notice.\n".to_vec(),
        ),
    ] {
        ensure!(
            !fixture.members.contains_key(name),
            "Obsolete fixture resource already belongs to the incoming package"
        );
        fixture.manifest.files.push(OwnedFile {
            relative_path: name.into(),
            role,
            size: bytes.len() as u64,
            sha256: archive::digest(&bytes),
        });
        fixture.members.insert(name.into(), bytes);
    }
    let manifest_bytes = serde_json::to_vec(&fixture.manifest)?;
    fixture.manifest.files.push(OwnedFile {
        relative_path: "support/manifest.json".into(),
        role: FileRole::Manifest,
        size: manifest_bytes.len() as u64,
        sha256: archive::digest(&manifest_bytes),
    });
    fixture
        .members
        .insert("support/manifest.json".into(), manifest_bytes);
    validate_manifest(&fixture.manifest)?;
    Ok(fixture)
}

fn msi_owner_case(options: &Options, records: &mut Vec<Value>) -> Result<()> {
    let details = (|| -> Result<Value> {
        let hash = runtime::observe_file(&options.package, "package")?
            .identity
            .sha256;
        let verified = archive::read_archive(&options.package, &hash)?;
        let portable = directory(&options.fixture_root, "portable")?;
        let portable_receipt =
            materialize_portable(&portable, &verified, &hash, env!("CARGO_PKG_VERSION"))?;
        let portable_names = portable_receipt
            .files
            .iter()
            .map(|file| file.relative_path.clone())
            .chain(std::iter::once(RECEIPT_PATH.into()))
            .collect::<Vec<_>>();
        let before = hashes(&portable, &portable_names)?;
        let library = directory(&options.fixture_root, "library")?;
        let context = runtime::DiscoveryContext {
            default_library: library.clone(),
            location_file: None,
            explicit_library: Some(library.clone()),
        };
        let registrations = runtime::registered_current_user_msi()?;
        ensure!(
            registrations.len() == 1 && registrations[0].version == "0.0.1",
            "MSI acceptance requires its fresh synthetic baseline only"
        );
        let installed = registrations[0].install_root.join("WW-StudyApp.exe");
        let receipt = runtime::discover(&installed, &context)
            .receipt
            .context("Synthetic MSI owner discovery failed")?;
        ensure!(
            matches!(receipt.owner, InstallOwner::Msi { .. })
                && runtime::discover(&portable.join("WW-StudyApp.exe"), &context)
                    .receipt
                    .is_some_and(|receipt| receipt.owner == InstallOwner::Portable),
            "Separate copies did not select their exact owners"
        );
        ensure!(
            prepare_portable_update(
                &options.package,
                &hash,
                &receipt,
                &ProtectedPaths::default(),
                env!("CARGO_PKG_VERSION")
            )
            .is_err(),
            "MSI resources accepted direct portable mutation"
        );
        ensure!(
            same_absolute_path(&std::env::current_exe()?, &installed)?,
            "Authenticated MSI test must run its owned installed ui-test image"
        );
        let msi_hash = runtime::observe_file(&options.msi, "package")?
            .identity
            .sha256;
        let mut prepared =
            prepare_msi_update(&options.msi, &msi_hash, &receipt, env!("CARGO_PKG_VERSION"))?;
        prepared.with_restart_data_directory(Some(&library))?;
        let plan = load_plan(&prepared)?;
        let captured_package = plan.msi_package.context("MSI package capture missing")?;
        ensure!(
            !absolute_components(&captured_package.path)?
                .starts_with(&absolute_components(&receipt.install_root)?),
            "MSI package was not staged externally"
        );
        let handoff = crate::maintenance_handoff::begin_update_handoff_for_acceptance(&prepared)?;
        create_new_file(
            &options.fixture_root.join("msi-handoff.json"),
            &serde_json::to_vec_pretty(&json!({
                "stage":handoff.stage,"helper_pid":handoff.helper_pid,"helper_created":handoff.helper_created,
                "transaction_id":prepared.transaction_id,"install_root":receipt.install_root,
                "portable_root":portable,"portable_hashes":before,"data_root":library,
                "package_path":captured_package.path,"package_identity":captured_package.identity,
                "source_product":receipt.owner,"expected_version":env!("CARGO_PKG_VERSION")
            }))?,
        )?;
        // Parent exit is part of actual authentication. The PowerShell consumer
        // must observe the real child result, health, payload, portable bytes and
        // process exit before issuing the success record.
        Ok(json!({"handoff_issued":true,"pending_authenticated_child":true}))
    })()?;
    records.push(json!({"scenario":"msi-handoff-issued","passed":false,"executor":"pending-child","details":details}));
    Ok(())
}

fn msi_cleanup_case(options: &Options, records: &mut Vec<Value>) -> Result<()> {
    record(records, "msi-owner-cleanup", || {
        let registrations = runtime::registered_current_user_msi()?;
        ensure!(
            registrations.len() == 1 && registrations[0].version == env!("CARGO_PKG_VERSION"),
            "Cleanup requires the exact current fixture MSI owner"
        );
        let root = registrations[0].install_root.clone();
        let default_root =
            PathBuf::from(std::env::var_os("LOCALAPPDATA").context("LOCALAPPDATA required")?)
                .join("Programs/WW-StudyApp");
        ensure!(
            same_absolute_path(&root, &default_root)?,
            "MSI cleanup is restricted to the fresh default fixture root"
        );
        let context = runtime::DiscoveryContext {
            default_library: options.fixture_root.join("unused-library"),
            location_file: None,
            explicit_library: Some(options.fixture_root.join("unused-library")),
        };
        let receipt = runtime::discover(&root.join("WW-StudyApp.exe"), &context)
            .receipt
            .context("Cleanup requires verified MSI payload")?;
        let mut confirmation = UninstallConfirmation::new(preview_uninstall(
            &receipt,
            DataSelection::default(),
            None,
            vec![],
        )?);
        confirmation.confirm_selection();
        let status = apply_authenticated_uninstall(prepare_uninstall(
            confirmation.finish()?,
            &receipt,
            &context,
        )?)?;
        ensure!(
            status.state == MaintenanceState::Succeeded
                && runtime::registered_current_user_msi()?.is_empty()
                && !root.try_exists()?,
            "Real default MSI cleanup left owner/control files"
        );
        Ok(json!({"status":status,"production_default_uninstall":true}))
    })
}

fn real_health_cases(options: &Options, records: &mut Vec<Value>) -> Result<()> {
    let hash = runtime::observe_file(&options.package, "package")?
        .identity
        .sha256;
    let verified = archive::read_archive(&options.package, &hash)?;
    record(records, "actual-storage-ready-health", || {
        let install = directory(&options.fixture_root, "real-upgrade")?;
        let baseline = upgrade_fixture_payload(&verified)?;
        let receipt = materialize_portable(&install, &baseline, &hash, &baseline.manifest.version)?;
        let data = directory(&options.fixture_root, "upgrade-data")?;
        create_new_file(
            &install.join("unknown-original.md"),
            b"retained unknown original",
        )?;
        let mut prepared = prepare_portable_update(
            &options.package,
            &hash,
            &receipt,
            &ProtectedPaths::default(),
            env!("CARGO_PKG_VERSION"),
        )?;
        prepared.with_restart_data_directory(Some(&data))?;
        let status = apply_portable_plan(&prepared)?;
        ensure!(
            status.state == MaintenanceState::Succeeded,
            "Real production upgrade/storage-ready acknowledgement failed"
        );
        let application = process::running_image(&install.join("WW-StudyApp.exe"))?;
        let pid = application.pid;
        ensure!(
            runtime::read_portable_receipt(&install.join("WW-StudyApp.exe"))?.version
                == env!("CARGO_PKG_VERSION"),
            "Healthy upgrade receipt version differs"
        );
        for resource in &verified.manifest.files {
            ensure!(
                runtime::observe_file(&install.join(&resource.relative_path), "healthy")?
                    .identity
                    .sha256
                    == resource.sha256,
                "Healthy upgraded payload differs"
            );
        }
        ensure!(
            !install.join("retired.dll").try_exists()?
                && std::fs::read(install.join("unknown-original.md"))?
                    == b"retained unknown original",
            "Real healthy update swept unknown content or retained obsolete owned runtime"
        );
        application.stop()?;
        Ok(
            json!({"status":status,"actual_child_pid":pid,"health":"production-version-nonce-and-storage-ready","child_cleanup":"retained-exact-process-handle","baseline_payload":"verified-current-package-with-synthetic-0.0.1-metadata"}),
        )
    })?;
    record(records, "locked-running-app", || {
        let install = directory(&options.fixture_root, "locked-application")?;
        let receipt = materialize_portable(&install, &verified, &hash, "0.0.1")?;
        let data = directory(&options.fixture_root, "locked-data")?;
        let record = receipt
            .files
            .iter()
            .find(|file| file.role == FileRole::Application)
            .context("Missing application resource")?;
        crate::maintenance_handoff::restart_and_wait_for_health(
            &install.join("WW-StudyApp.exe"),
            record,
            env!("CARGO_PKG_VERSION"),
            &[
                "--data-dir".into(),
                data.to_str().context("Fixture path encoding")?.into(),
            ],
        )?;
        let application = process::running_image(&install.join("WW-StudyApp.exe"))?;
        let names = receipt
            .files
            .iter()
            .map(|file| file.relative_path.clone())
            .chain(std::iter::once(RECEIPT_PATH.into()))
            .collect::<Vec<_>>();
        let before = hashes(&install, &names)?;
        let prepared = prepare_portable_update(
            &options.package,
            &hash,
            &receipt,
            &ProtectedPaths::default(),
            env!("CARGO_PKG_VERSION"),
        )?;
        // The real app is running. Hold an explicit runtime read lease to make
        // the in-use file refusal deterministic without closing another process.
        let dll =
            crate::storage_backend::plain_handle(&install.join("vcruntime140.dll"), false, true)?;
        let mut called = false;
        let status = apply_with_health(
            &prepared,
            |_| {
                called = true;
                Ok(())
            },
            |_, _| Ok(()),
        )?;
        ensure!(
            status.state == MaintenanceState::RolledBack
                && !called
                && application.running()?
                && hashes(&install, &names)? == before,
            "Running/locked app was forced through replacement or not restored"
        );
        drop(dll);
        let pid = application.pid;
        application.stop()?;
        Ok(
            json!({"status":status,"actual_running_pid":pid,"actual_storage_ready_before_lock":true,"runtime_lease_denied_delete":true,"restart_callback_not_reached":true}),
        )
    })
}

fn staged_uninstall(options: &Options, records: &mut Vec<Value>) -> Result<()> {
    let executable = std::env::current_exe()?;
    let install = executable
        .parent()
        .context("Missing acceptance install root")?;
    directory(install, "support")?;
    let mut resources = Vec::new();
    for (name, role) in [
        ("WW-StudyApp.exe", FileRole::Application),
        ("Uninstall.exe", FileRole::Uninstaller),
        ("vcruntime140.dll", FileRole::Runtime),
        ("vcruntime140_1.dll", FileRole::Runtime),
    ] {
        let actual = runtime::observe_file(&install.join(name), name)?;
        resources.push(OwnedFile {
            relative_path: name.into(),
            role,
            size: actual.identity.size,
            sha256: actual.identity.sha256,
        });
    }
    for name in ["support/LICENSE", "support/THIRD-PARTY-NOTICES.txt"] {
        let actual = create_new_file(
            &install.join(name),
            b"Private transient acceptance fixture notice",
        )?;
        resources.push(OwnedFile {
            relative_path: name.into(),
            role: FileRole::Notice,
            size: actual.size,
            sha256: actual.sha256,
        });
    }
    let manifest = PackageManifest {
        schema: 1,
        product: PRODUCT.into(),
        architecture: ARCHITECTURE.into(),
        version: env!("CARGO_PKG_VERSION").into(),
        files: resources.clone(),
    };
    let manifest_identity = create_new_file(
        &install.join("support/manifest.json"),
        &serde_json::to_vec(&manifest)?,
    )?;
    resources.push(OwnedFile {
        relative_path: "support/manifest.json".into(),
        role: FileRole::Manifest,
        size: manifest_identity.size,
        sha256: manifest_identity.sha256,
    });
    let receipt = InstallReceipt {
        schema: 1,
        install_id: uuid::Uuid::new_v4().to_string(),
        owner: InstallOwner::Portable,
        install_root: install.into(),
        root_identity: runtime::handle_identity(
            &crate::storage_backend::plain_handle(install, true, false)?,
            true,
        )?
        .0,
        version: env!("CARGO_PKG_VERSION").into(),
        source_package_sha256: "b".repeat(64),
        files: resources,
    };
    create_new_file(&install.join(RECEIPT_PATH), &serde_json::to_vec(&receipt)?)?;
    let fixture = data_fixture(&options.fixture_root, "staged-data")?;
    let discovery = runtime::discover_library(&fixture.context);
    let mut confirmation = UninstallConfirmation::new(preview_uninstall(
        &receipt,
        DataSelection::default(),
        discovery.verified_database,
        discovery.configuration_files,
    )?);
    confirmation.confirm_selection();
    let intent = prepare_uninstall(confirmation.finish()?, &receipt, &fixture.context)?;
    let transaction = intent.transaction_id.clone();
    let handoff = crate::maintenance_handoff::begin_uninstall_handoff_for_acceptance(intent)?;
    let data=std::iter::once(fixture.database.clone()).chain(fixture.configuration.iter().map(|(path,_)|path.clone())).map(|path|Ok(json!({"path":path,"sha256":runtime::observe_file(&path,"preserved")?.identity.sha256}))).collect::<Result<Vec<_>>>()?;
    create_new_file(
        &options.fixture_root.join("staged-handoff.json"),
        &serde_json::to_vec_pretty(
            &json!({"stage":handoff.stage,"helper_pid":handoff.helper_pid,"helper_created":handoff.helper_created,"transaction_id":transaction,"install_root":install,"data":data,"owned_paths":receipt.files.iter().map(|file|file.relative_path.clone()).collect::<Vec<_>>() }),
        )?,
    )?;
    // This records only issuance. The script must wait for parent exit/result,
    // verify removed EXE/DLLs and preserved data, then report actual success.
    records.push(json!({"scenario":"staged-handoff-issued","passed":false,"executor":"pending-child","transaction_id":transaction}));
    Ok(())
}

#[cfg(test)]
mod cli_tests {
    use super::*;

    fn memory_create_new(
        files: &mut BTreeMap<String, Vec<u8>>,
        name: &str,
        bytes: &[u8],
    ) -> Result<FileIdentity> {
        ensure!(!files.contains_key(name), "create_new conflict: {name}");
        files.insert(name.into(), bytes.to_vec());
        Ok(FileIdentity {
            volume: 1,
            file_id: files.len() as u128,
            size: bytes.len() as u64,
            sha256: archive::digest(bytes),
        })
    }

    #[test]
    fn maintenance_recovery_fixture_manifest_is_unique_and_matches_receipt() {
        let source = memory_package();
        let mut old_bytes = source.members.clone();
        for name in [
            "WW-StudyApp.exe",
            "Uninstall.exe",
            "vcruntime140.dll",
            "vcruntime140_1.dll",
        ] {
            old_bytes.insert(
                name.into(),
                format!("pure old metadata: {name}").into_bytes(),
            );
        }
        let mut created = BTreeMap::new();
        let mut reads = Vec::new();
        let receipt_files = materialize_recovery_fixture(
            &source,
            |resource| {
                reads.push(resource.role);
                Ok(old_bytes[&resource.relative_path].clone())
            },
            |name, bytes| memory_create_new(&mut created, name, bytes),
        )
        .expect("recovery fixture must create each resource once");
        assert!(!reads.contains(&FileRole::Manifest));
        assert_eq!(created.len(), 7);
        assert_eq!(receipt_files.len(), 7);
        assert_eq!(
            receipt_files
                .iter()
                .filter(|file| file.role == FileRole::Manifest)
                .count(),
            1
        );
        let manifest: PackageManifest =
            serde_json::from_slice(&created["support/manifest.json"]).unwrap();
        validate_manifest(&manifest).unwrap();
        assert_eq!(manifest.version, "0.0.1");
        assert_eq!(manifest.files.len(), 6);
        assert!(
            manifest
                .files
                .iter()
                .all(|file| file.role != FileRole::Manifest)
        );
        assert_eq!(
            manifest.files,
            receipt_files
                .iter()
                .filter(|file| file.role != FileRole::Manifest)
                .cloned()
                .collect::<Vec<_>>()
        );
        for file in &receipt_files {
            let bytes = &created[&file.relative_path];
            assert_eq!(file.size, bytes.len() as u64);
            assert_eq!(file.sha256, archive::digest(bytes));
            if file.role != FileRole::Manifest {
                assert_eq!(bytes, &old_bytes[&file.relative_path]);
                assert_eq!(
                    file.role,
                    source
                        .manifest
                        .files
                        .iter()
                        .find(|source_file| source_file.relative_path == file.relative_path)
                        .unwrap()
                        .role
                );
            }
        }
        let before_repeat = created.clone();
        assert!(
            materialize_recovery_fixture(
                &source,
                |resource| Ok(old_bytes[&resource.relative_path].clone()),
                |name, bytes| memory_create_new(&mut created, name, bytes),
            )
            .is_err()
        );
        assert_eq!(created, before_repeat);
    }

    #[test]
    fn maintenance_recovery_fixture_preserves_existing_manifest_on_conflict() {
        let source = memory_package();
        let original = b"preexisting manifest must not be overwritten".to_vec();
        let mut created = BTreeMap::from([("support/manifest.json".into(), original.clone())]);
        let error = materialize_recovery_fixture(
            &source,
            |resource| Ok(source.members[&resource.relative_path].clone()),
            |name, bytes| memory_create_new(&mut created, name, bytes),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("create_new conflict: support/manifest.json")
        );
        assert_eq!(created["support/manifest.json"], original);
    }

    #[test]
    fn maintenance_legacy_report_accepts_plain_json_and_one_leading_utf8_bom() {
        let plain = "{\"schema\":1,\"note\":\"保留原件\"}\r\n".as_bytes();
        let expected = json!({"schema":1,"note":"保留原件"});
        assert_eq!(parse_legacy_acceptance_report(plain).unwrap(), expected);
        let mut with_bom = b"\xEF\xBB\xBF".to_vec();
        with_bom.extend_from_slice(plain);
        assert_eq!(parse_legacy_acceptance_report(&with_bom).unwrap(), expected);
    }

    #[test]
    fn maintenance_legacy_report_rejects_invalid_json_extra_bom_and_non_utf8() {
        for bytes in [
            b"".as_slice(),
            b"{".as_slice(),
            b"\xEF\xBB\xBF{".as_slice(),
            b"\xEF\xBB\xBF\xEF\xBB\xBF{}".as_slice(),
            b" \xEF\xBB\xBF{}".as_slice(),
            b"{\"note\":\"\xFF\"}".as_slice(),
            b"\xEF\xBB\xBF{\"note\":\"\xFF\"}".as_slice(),
            b"{} {}".as_slice(),
        ] {
            assert!(
                parse_legacy_acceptance_report(bytes).is_err(),
                "accepted malformed legacy report: {bytes:?}"
            );
        }
    }

    // Metadata-only inputs: these bytes are never written or executed. Native
    // acceptance obtains its actual EXE/DLL payload through read_archive.
    fn memory_package() -> archive::VerifiedArchive {
        let mut members = BTreeMap::new();
        let mut files = Vec::new();
        for (name, role) in [
            ("WW-StudyApp.exe", FileRole::Application),
            ("Uninstall.exe", FileRole::Uninstaller),
            ("vcruntime140.dll", FileRole::Runtime),
            ("vcruntime140_1.dll", FileRole::Runtime),
            ("support/LICENSE", FileRole::Notice),
            ("support/THIRD-PARTY-NOTICES.txt", FileRole::Notice),
        ] {
            let bytes = format!("pure metadata input: {name}").into_bytes();
            files.push(OwnedFile {
                relative_path: name.into(),
                role,
                size: bytes.len() as u64,
                sha256: archive::digest(&bytes),
            });
            members.insert(name.into(), bytes);
        }
        let mut manifest = PackageManifest {
            schema: 1,
            product: PRODUCT.into(),
            architecture: ARCHITECTURE.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            files,
        };
        let bytes = serde_json::to_vec(&manifest).unwrap();
        manifest.files.push(OwnedFile {
            relative_path: "support/manifest.json".into(),
            role: FileRole::Manifest,
            size: bytes.len() as u64,
            sha256: archive::digest(&bytes),
        });
        members.insert("support/manifest.json".into(), bytes);
        archive::VerifiedArchive { manifest, members }
    }

    #[test]
    fn maintenance_upgrade_fixture_preserves_complete_payload_and_consistent_manifest() {
        let source = memory_package();
        let baseline = upgrade_fixture_payload(&source).unwrap();
        for file in source
            .manifest
            .files
            .iter()
            .filter(|file| file.role != FileRole::Manifest)
        {
            assert!(
                baseline.manifest.files.contains(file),
                "baseline lost verified role/identity: {}",
                file.relative_path
            );
            assert_eq!(
                baseline.members[&file.relative_path],
                source.members[&file.relative_path]
            );
        }
        assert_eq!(baseline.manifest.version, "0.0.1");
        assert_eq!(
            baseline.manifest.files.len(),
            source.manifest.files.len() + 2
        );
        assert_eq!(
            baseline.members["retired.dll"],
            source.members["vcruntime140.dll"]
        );
        validate_manifest(&baseline.manifest).unwrap();
        for file in &baseline.manifest.files {
            let bytes = &baseline.members[&file.relative_path];
            assert_eq!(file.size, bytes.len() as u64);
            assert_eq!(file.sha256, archive::digest(bytes));
        }
        let disk_manifest: PackageManifest =
            serde_json::from_slice(&baseline.members["support/manifest.json"]).unwrap();
        assert_eq!(disk_manifest.version, baseline.manifest.version);
        assert_eq!(
            disk_manifest.files,
            baseline
                .manifest
                .files
                .iter()
                .filter(|file| file.role != FileRole::Manifest)
                .cloned()
                .collect::<Vec<_>>()
        );
        assert_eq!(source.manifest.version, env!("CARGO_PKG_VERSION"));
        assert!(!source.members.contains_key("retired.dll"));
    }

    #[test]
    fn maintenance_upgrade_fixture_retains_changed_notice_and_removes_owned_old_runtime() {
        let source = memory_package();
        let baseline = upgrade_fixture_payload(&source).unwrap();
        let receipt = InstallReceipt {
            schema: 1,
            install_id: "pure-upgrade-fixture".into(),
            owner: InstallOwner::Portable,
            install_root: PathBuf::from(r"C:\isolated\pure-fixture"),
            root_identity: FileIdentity {
                volume: 1,
                file_id: 2,
                size: 0,
                sha256: String::new(),
            },
            version: baseline.manifest.version.clone(),
            source_package_sha256: "a".repeat(64),
            files: baseline.manifest.files,
        };
        let mut observed = receipt
            .files
            .iter()
            .enumerate()
            .map(|(index, file)| ObservedFile {
                relative_path: file.relative_path.clone(),
                identity: FileIdentity {
                    volume: 1,
                    file_id: index as u128 + 10,
                    size: file.size,
                    sha256: file.sha256.clone(),
                },
                is_reparse: false,
                link_count: 1,
            })
            .collect::<Vec<_>>();
        observed
            .iter_mut()
            .find(|file| file.relative_path == "support/README.md")
            .expect("baseline needs an obsolete non-runtime notice")
            .identity
            .sha256 = "b".repeat(64);
        observed.push(ObservedFile {
            relative_path: "unknown-source.md".into(),
            identity: FileIdentity {
                volume: 1,
                file_id: 99,
                size: 3,
                sha256: "c".repeat(64),
            },
            is_reparse: false,
            link_count: 1,
        });
        let plan = plan_upgrade(
            &receipt,
            &source.manifest,
            &observed,
            &ProtectedPaths::default(),
        )
        .unwrap();
        assert_eq!(plan.retained_conflicts, ["support/README.md"]);
        assert_eq!(plan.operations.len(), source.manifest.files.len() + 1);
        assert!(plan.operations.iter().any(|operation| matches!(operation,
            FileOperation::RemoveOwned { old, .. } if old.relative_path == "retired.dll")));
        for resource in receipt.files.iter().filter(|file| {
            matches!(
                file.role,
                FileRole::Application | FileRole::Uninstaller | FileRole::Runtime
            )
        }) {
            let actual = observed
                .iter()
                .find(|file| file.relative_path == resource.relative_path)
                .unwrap();
            assert_eq!(
                actual.identity.sha256, resource.sha256,
                "recovery source must stay intact"
            );
        }
    }
    fn arguments(mode: &str) -> Vec<String> {
        [
            mode,
            "--fixture-root",
            r"C:\ci\fresh",
            "--package",
            r"C:\input\portable.zip",
            "--msi",
            r"C:\input\app.msi",
            "--evidence",
            r"C:\ci\fresh\maintenance-execution.json",
            "--data-dir",
            r"C:\ci\fresh\fallback-data",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }
    #[test]
    fn maintenance_acceptance_cli_consumes_mode_without_executable_argument() {
        let normal = parse_options(&arguments("--maintenance-execution-smoke"))
            .expect("main passes args.skip(1), so element zero is the mode");
        assert!(!normal.staged);
        assert_eq!(normal.suite, "journal");
        assert!(
            parse_options(&arguments("--maintenance-staged-uninstall-smoke"))
                .unwrap()
                .staged
        );
        let mut with_exe = arguments("--maintenance-execution-smoke");
        with_exe.insert(0, r"C:\ci\driver.exe".into());
        assert!(parse_options(&with_exe).is_err());
    }
    #[test]
    fn maintenance_acceptance_cli_accepts_recovery_without_executable_argument() {
        for suite in ["recovery-prepare-missing", "recovery-prepare-mixed"] {
            let mut args = arguments("--maintenance-execution-smoke");
            args.extend(
                ["--suite", suite, "--recovery-runtime", r"C:\runtime"]
                    .into_iter()
                    .map(str::to_owned),
            );
            let options = parse_options(&args).expect("recovery mode is the first real argument");
            assert_eq!(options.suite, suite);
            assert_eq!(options.recovery_runtime, Some(PathBuf::from(r"C:\runtime")));
        }
        let mut args = arguments("--maintenance-execution-smoke");
        args.extend(["--suite".into(), "recovery-ui".into()]);
        assert_eq!(parse_options(&args).unwrap().suite, "recovery-ui");
    }
    #[test]
    fn maintenance_acceptance_cli_rejects_duplicate_and_missing_modes() {
        for mode in [
            "--maintenance-execution-smoke",
            "--maintenance-staged-uninstall-smoke",
        ] {
            let mut args = arguments(mode);
            args.insert(0, "--maintenance-execution-smoke".into());
            assert!(
                parse_options(&args).is_err(),
                "duplicate initial mode must not be skipped"
            );
        }
        let mut args = arguments("--maintenance-execution-smoke");
        args.remove(0);
        assert!(parse_options(&args).is_err());
        let mut args = arguments("--maintenance-execution-smoke");
        args.extend(["--package".into(), "duplicate.zip".into()]);
        assert!(parse_options(&args).is_err());
    }
}
