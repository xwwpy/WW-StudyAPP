use crate::preferences::{PreferenceSnapshot, PreferencesStore, SavePreferences};
use crate::storage_backend::StorageEvent;
use std::{
    path::PathBuf,
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};
use studyapp_core::{Command, Engine, Snapshot};

#[path = "startup_error.rs"]
pub mod startup;
use startup::{StartupFailure, StartupStage};

// Test-only boundary lines correlate startup, receive deadlines and Drop joins.
// Normal builds expand the original expressions without a probe or closure.
macro_rules! startup_step {
    (@phase result, $result:ident) => { if $result.is_ok() { "exit" } else { "error" } };
    (@phase value, $value:ident) => { "exit" };
    ($kind:ident, $stage:ident, $operation:expr) => {{
        #[cfg(test)]
        {
            startup_timing::event(stringify!($stage), "enter");
            let result = $operation;
            startup_timing::event(stringify!($stage), startup_step!(@phase $kind, result));
            result
        }
        #[cfg(not(test))]
        { $operation }
    }};
}

#[cfg(test)]
mod startup_timing {
    use std::{
        cell::Cell,
        sync::atomic::{AtomicU64, Ordering},
        thread,
        time::Instant,
    };
    #[derive(Clone, Copy)]
    pub struct Probe {
        id: u64,
        created: Instant,
    }
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    thread_local! { static CURRENT: Cell<Option<(Probe,u64)>> = const { Cell::new(None) }; }
    impl Probe {
        pub fn new() -> Self {
            let probe = Self {
                id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
                created: Instant::now(),
            };
            probe.observe("Spawn", "enter");
            probe
        }
        fn line(self, stage: &str, phase: &str, attempt: impl std::fmt::Display) {
            eprintln!(
                "worker-startup id={} attempt={} elapsed={}us thread={:?} stage={} phase={}",
                self.id,
                attempt,
                self.created.elapsed().as_micros(),
                thread::current().id(),
                stage,
                phase
            );
        }
        pub fn observe(self, stage: &str, phase: &str) {
            self.line(stage, phase, "observer");
        }
    }
    pub fn thread_start(probe: Probe) {
        CURRENT.with(|current| current.set(Some((probe, 0))));
        event("Thread", "enter");
    }
    pub fn attempt() {
        CURRENT.with(|current| {
            if let Some((probe, n)) = current.get() {
                current.set(Some((probe, n + 1)));
            }
        });
        event("OpenAttempt", "enter");
    }
    pub fn event(stage: &str, phase: &str) {
        CURRENT.with(|current| {
            if let Some((probe, n)) = current.get() {
                probe.line(stage, phase, n);
            }
        });
    }
}

pub enum Work {
    Scoped {
        library: String,
        work: Box<Work>,
    },
    Apply(Command),
    SavePreferences(SavePreferences),
    Media(crate::media::MediaRequest),
    TestImageCloud,
    RetryImageCloud,
    ImageCloudFinished {
        generation: u64,
        revision: String,
        result: Result<String, String>,
    },
    ImportPaths(Vec<PathBuf>),
    PreparedImport {
        generation: u64,
        selection: crate::imports::ImportSelection,
    },
    StartSync {
        test_only: bool,
    },
    SyncFinished {
        generation: u64,
        result: Result<crate::sync_runtime::SyncCompletion, String>,
    },
    SwitchLibrary(PathBuf),
    RetryOpenLibrary,
    RefreshStorage,
    DeleteManagedCopy {
        note_id: String,
    },
    MigrateLibrary(PathBuf),
    Shutdown,
}

#[derive(Default)]
pub struct Update {
    pub startup_failure: Option<StartupFailure>,
    pub snapshot: Option<Arc<Snapshot>>,
    pub read_models: Option<Arc<studyapp_desktop::read_models::SnapshotReadModels>>,
    pub notice: Option<Result<String, String>>,
    pub editor_result: bool,
    pub library_replaced: bool,
    pub library_result: Option<Result<(), String>>,
    pub note_result: Option<Result<String, String>>,
    pub sync_status: Option<crate::sync_runtime::SyncStatus>,
    pub storage_event: Option<StorageEvent>,
    pub preferences: Option<PreferenceSnapshot>,
    pub preferences_result: Option<Result<(), String>>,
    pub media_result: Option<crate::media::MediaResponse>,
    pub image_cloud_busy: Option<bool>,
}

pub struct Worker {
    pub sender: mpsc::Sender<Work>,
    pub receiver: mpsc::Receiver<Update>,
    handle: Option<thread::JoinHandle<()>>,
    #[cfg(test)]
    startup_probe: startup_timing::Probe,
}

fn launch_sync(
    runtime: &mut crate::sync_runtime::SyncRuntime,
    engine: &Engine,
    sender: &mpsc::Sender<Work>,
    generation: u64,
    test_only: bool,
) -> anyhow::Result<()> {
    let job = runtime.task(Arc::new(engine.snapshot()), test_only)?;
    let sender = sender.clone();
    thread::spawn(move || {
        let result = job().map_err(|error| error.to_string());
        let _ = sender.send(Work::SyncFinished { generation, result });
    });
    Ok(())
}

fn open_library(
    path: &std::path::Path,
    now: i64,
) -> Result<
    (
        std::fs::File,
        Engine,
        PreferencesStore,
        crate::sync_runtime::SyncRuntime,
    ),
    StartupFailure,
> {
    #[cfg(test)]
    startup_timing::attempt();
    if let Some(parent) = path.parent() {
        startup_step!(
            result,
            Directory,
            std::fs::create_dir_all(parent).map_err(|error| {
                StartupFailure::from_error(
                    path,
                    StartupStage::PrepareDirectory,
                    parent,
                    &error.into(),
                )
            })
        )?;
    }
    let lock_path = path.with_extension("lock");
    let lock = startup_step!(
        result,
        LockOpen,
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|error| {
                StartupFailure::from_error(
                    path,
                    StartupStage::LibraryLock,
                    &lock_path,
                    &error.into(),
                )
            })
    )?;
    startup_step!(
        result,
        LockAcquire,
        lock.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => StartupFailure::in_use(path, &lock_path),
            std::fs::TryLockError::Error(error) => {
                StartupFailure::from_error(
                    path,
                    StartupStage::LibraryLock,
                    &lock_path,
                    &error.into(),
                )
            }
        })
    )?;
    let mut engine = startup_step!(
        result,
        Engine,
        Engine::open(path, now).map_err(|error| StartupFailure::from_error(
            path,
            StartupStage::Database,
            path,
            &error
        ))
    )?;
    let database = PathBuf::from(engine.snapshot().data_path);
    let settings_path = database.parent().unwrap_or(path).join("settings");
    let (preferences, synchronization) = startup_step!(
        result,
        Preferences,
        PreferencesStore::open(&database, &mut engine).map_err(|error| {
            StartupFailure::from_error(path, StartupStage::Preferences, &settings_path, &error)
        })
    )?;
    #[cfg(test)]
    startup_timing::event("OpenAttempt", "exit");
    Ok((lock, engine, preferences, synchronization))
}

fn save_scoped_settings(
    command: Command,
    preferences: &mut PreferencesStore,
    synchronization: &mut crate::sync_runtime::SyncRuntime,
    engine: &mut Engine,
    now: i64,
) -> anyhow::Result<String> {
    let current = preferences.snapshot();
    let mut settings = current.settings;
    match command {
        Command::SaveSettings(next) => settings = next,
        Command::SaveLearningSettings {
            interval_minutes,
            answer_minutes,
            review_days,
        } => {
            settings.interval_minutes = interval_minutes;
            settings.answer_minutes = answer_minutes;
            settings.review_days = review_days;
        }
        Command::SetShowFilePaths(visible) => settings.show_file_paths = visible,
        Command::SaveLayout(layout) => settings.layout = layout,
        Command::Restore(path) => return engine.restore_preserving_settings(&path, now),
        other => return engine.apply(other, now),
    }
    preferences.save(
        SavePreferences {
            expected_revision: current.revision,
            settings,
            shortcuts: current.shortcuts,
            sync: current.sync,
            password: None,
            image_secret: Default::default(),
        },
        engine,
        synchronization,
    )?;
    Ok("设置已保存".into())
}

impl Worker {
    pub fn start(path: PathBuf, location_config: Option<PathBuf>) -> Self {
        let (sender, commands) = mpsc::channel();
        let (updates, receiver) = mpsc::channel();
        let command_sender = sender.clone();
        #[cfg(test)]
        let startup_probe = startup_timing::Probe::new();
        let handle = thread::spawn(move || {
            #[cfg(test)]
            startup_timing::thread_start(startup_probe);
            let mut requested_path = path;
            let mut recovery_requested = false;
            let mut remember_location = false;
            let (mut library_lock, mut engine, mut preferences, mut synchronization) = loop {
                let opened = open_library(&requested_path, chrono::Utc::now().timestamp())
                    .and_then(|library| {
                        if remember_location && let Some(config) = &location_config {
                            let snapshot = library.1.snapshot();
                            let directory = std::path::Path::new(&snapshot.data_path)
                                .parent()
                                .expect("opened database has a parent");
                            crate::storage_backend::persist_location(config, directory).map_err(
                                |error| {
                                    StartupFailure::from_error(
                                        &requested_path,
                                        StartupStage::RememberLocation,
                                        config,
                                        &error,
                                    )
                                },
                            )?;
                        }
                        Ok(library)
                    });
                let failure = match opened {
                    Ok(library) => break library,
                    Err(failure) => failure,
                };
                let failure_update = startup_step!(
                    value,
                    ErrorUpdate,
                    Update {
                        notice: Some(Err(failure.to_string())),
                        library_result: recovery_requested.then(|| Err(failure.to_string())),
                        startup_failure: Some(failure.clone()),
                        ..Default::default()
                    }
                );
                if startup_step!(result, ErrorSend, updates.send(failure_update)).is_err() {
                    return;
                }
                // A failed open has no active Engine. Keep the command channel
                // available for explicit recovery, without creating a fallback library.
                loop {
                    match commands.recv() {
                        Ok(Work::RetryOpenLibrary) => {
                            recovery_requested = true;
                            break;
                        }
                        Ok(Work::SwitchLibrary(directory)) => {
                            requested_path = directory.join("studyapp.sqlite3");
                            recovery_requested = true;
                            remember_location = true;
                            break;
                        }
                        Ok(Work::Shutdown) | Err(_) => {
                            #[cfg(test)]
                            startup_timing::event("ShutdownOrDisconnect", "enter");
                            return;
                        }
                        Ok(_) => {
                            if updates
                                .send(Update {
                                    startup_failure: Some(failure.clone()),
                                    notice: Some(Err(
                                        "资料库尚未打开，请先重试或选择其他资料库目录。".into(),
                                    )),
                                    ..Default::default()
                                })
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
                }
            };
            let mut generation = 0_u64;
            let mut read_model_cache = studyapp_desktop::read_models::ReadModelCache::default();
            let mut image_cloud_busy = false;
            let mut last_sync_attempt = Instant::now();
            let snapshot = startup_step!(value, Snapshot, Arc::new(engine.snapshot()));
            let read_models = startup_step!(
                value,
                ReadModels,
                read_model_cache.refresh(&snapshot, &chrono::Local, chrono::Utc::now().timestamp())
            );
            let _ = startup_step!(
                result,
                SnapshotSend,
                updates.send(Update {
                    startup_failure: None,
                    snapshot: Some(snapshot),
                    read_models: Some(read_models),
                    notice: None,
                    editor_result: false,
                    library_replaced: recovery_requested,
                    library_result: recovery_requested.then_some(Ok(())),
                    note_result: None,
                    sync_status: Some(synchronization.status.clone()),
                    storage_event: None,
                    preferences: Some(preferences.snapshot()),
                    preferences_result: None,
                    media_result: None,
                    image_cloud_busy: Some(image_cloud_busy),
                })
            );
            let mut last_tick = Instant::now();
            let mut last_scan = Instant::now();
            let mut last_checkpoint = Instant::now();
            loop {
                let work = commands.recv_timeout(Duration::from_millis(150));
                let work = match work {
                    Ok(Work::Scoped { library, work }) => {
                        if library != engine.snapshot().data_path {
                            let error = "资料库已切换，旧窗口的操作未执行；请在当前资料库重新操作"
                                .to_owned();
                            let editor_result = matches!(
                                work.as_ref(),
                                Work::Apply(
                                    Command::Submit { .. }
                                        | Command::Manual { .. }
                                        | Command::EditSession { .. }
                                )
                            );
                            let note_result = matches!(
                                work.as_ref(),
                                Work::Apply(
                                    Command::CreateNote { .. }
                                        | Command::CreateNoteInFolder { .. }
                                        | Command::CreateNoteWithId { .. }
                                        | Command::SaveNote { .. }
                                )
                            )
                            .then(|| Err(error.clone()));
                            let storage_event = matches!(
                                work.as_ref(),
                                Work::RefreshStorage
                                    | Work::DeleteManagedCopy { .. }
                                    | Work::MigrateLibrary(_)
                            )
                            .then(|| StorageEvent::Finished(Err(error.clone())));
                            let preferences_result =
                                matches!(work.as_ref(), Work::SavePreferences(_))
                                    .then(|| Err(error.clone()));
                            let library_result = matches!(
                                work.as_ref(),
                                Work::SwitchLibrary(_) | Work::Apply(Command::Restore(_))
                            )
                            .then(|| Err(error.clone()));
                            let media_result = if let Work::Media(request) = work.as_ref() {
                                Some(crate::media::MediaResponse {
                                    library: library.clone(),
                                    generation: request.generation(),
                                    request: request.request(),
                                    result: Err(error.clone()),
                                })
                            } else {
                                None
                            };
                            let snapshot = Arc::new(engine.snapshot());
                            let read_models = read_model_cache.refresh(
                                &snapshot,
                                &chrono::Local,
                                chrono::Utc::now().timestamp(),
                            );
                            let _ = updates.send(Update {
                                startup_failure: None,
                                snapshot: Some(snapshot),
                                read_models: Some(read_models),
                                notice: Some(Err(error)),
                                editor_result,
                                library_replaced: false,
                                library_result,
                                note_result,
                                sync_status: None,
                                storage_event,
                                preferences: Some(preferences.snapshot()),
                                preferences_result,
                                media_result,
                                image_cloud_busy: Some(image_cloud_busy),
                            });
                            continue;
                        }
                        Ok(*work)
                    }
                    other => other,
                };
                let now = chrono::Utc::now().timestamp();
                let mut notice = None;
                let mut changed = false;
                let mut editor_result = false;
                let mut library_replaced = false;
                let mut library_result = None;
                let mut note_result = None;
                let mut sync_changed = false;
                let mut storage_event = None;
                let mut preferences_result = None;
                match work {
                    Ok(Work::Media(request)) => {
                        let library = engine.snapshot().data_path;
                        let factory = synchronization.image_provider_factory(request.settings());
                        let updates = updates.clone();
                        thread::spawn(move || {
                            let generation = request.generation();
                            let request_id = request.request();
                            let result = {
                                // Even unavailable credentials preserve imported clipboard
                                // bytes in a staged local file before reporting failure.
                                crate::media::execute_with_provider_factory(
                                    std::path::Path::new(&library),
                                    request,
                                    factory,
                                )
                            }
                            .map_err(|error| format!("{error:#}"));
                            let _ = updates.send(Update {
                                startup_failure: None,
                                snapshot: None,
                                read_models: None,
                                notice: None,
                                editor_result: false,
                                library_replaced: false,
                                library_result: None,
                                note_result: None,
                                sync_status: None,
                                storage_event: None,
                                preferences: None,
                                preferences_result: None,
                                media_result: Some(crate::media::MediaResponse {
                                    library,
                                    generation,
                                    request: request_id,
                                    result,
                                }),
                                image_cloud_busy: None,
                            });
                        });
                    }
                    Ok(action @ (Work::TestImageCloud | Work::RetryImageCloud)) => {
                        if image_cloud_busy {
                            notice = Some(Err("图片云端任务正在进行，请稍候。".into()));
                        } else {
                            let snapshot = Arc::new(engine.snapshot());
                            let factory =
                                synchronization.image_provider_factory(&snapshot.settings.images);
                            let revision = preferences.snapshot().revision;
                            let sender = command_sender.clone();
                            let test_only = matches!(action, Work::TestImageCloud);
                            image_cloud_busy = true;
                            thread::spawn(move || {
                                let result = (|| -> anyhow::Result<String> {
                                    if test_only {
                                        let provider = factory()?.ok_or_else(|| {
                                            anyhow::anyhow!("请先配置图片云端服务并保存设置")
                                        })?;
                                        provider.check_connection()?;
                                        Ok("图片云端连接检查成功；上传权限将在实际上传时验证。"
                                            .into())
                                    } else {
                                        crate::image_cloud::retry_with_provider(
                                            &snapshot,
                                            factory(),
                                        )
                                    }
                                })()
                                .map_err(|error| error.to_string());
                                let _ = sender.send(Work::ImageCloudFinished {
                                    generation,
                                    revision,
                                    result,
                                });
                            });
                        }
                        changed = true;
                    }
                    Ok(Work::ImageCloudFinished {
                        generation: completed_generation,
                        revision,
                        result,
                    }) => {
                        image_cloud_busy = false;
                        if completed_generation == generation
                            && preferences.snapshot().revision == revision
                        {
                            notice = Some(result);
                        }
                        changed = true;
                    }
                    Ok(Work::Scoped { .. }) => continue,
                    Ok(Work::RetryOpenLibrary) => continue,
                    Ok(Work::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                        #[cfg(test)]
                        startup_timing::event("ShutdownOrDisconnect", "enter");
                        if let Err(error) =
                            startup_step!(result, ShutdownCheckpoint, engine.checkpoint())
                        {
                            eprintln!("保存退出状态失败：{error:#}");
                        }
                        break;
                    }
                    Ok(Work::Apply(command)) => {
                        let existing_notes = if matches!(
                            &command,
                            Command::CreateNote { .. }
                                | Command::CreateNoteInFolder { .. }
                                | Command::CreateNoteWithId { .. }
                        ) {
                            Some(
                                engine
                                    .snapshot()
                                    .notes
                                    .into_iter()
                                    .map(|n| n.id)
                                    .collect::<std::collections::HashSet<_>>(),
                            )
                        } else {
                            None
                        };
                        let saved_note = match &command {
                            Command::SaveNote { note_id, .. } => Some(note_id.clone()),
                            _ => None,
                        };
                        editor_result = matches!(
                            &command,
                            Command::Submit { .. }
                                | Command::Manual { .. }
                                | Command::EditSession { .. }
                        );
                        let restoring = matches!(&command, Command::Restore(_));
                        let result = if (synchronization.status.busy || image_cloud_busy)
                            && (restoring || matches!(&command, Command::UnmanageNote { .. }))
                        {
                            Err(if restoring {
                                "同步正在运行，请等待完成后再恢复资料库"
                            } else {
                                "同步正在运行，请等待完成后再取消管理笔记"
                            }
                            .into())
                        } else {
                            save_scoped_settings(
                                command,
                                &mut preferences,
                                &mut synchronization,
                                &mut engine,
                                now,
                            )
                            .map_err(|e| format!("{e:#}"))
                        };
                        if existing_notes.is_some() || saved_note.is_some() {
                            note_result = Some(match &result {
                                Ok(_) => saved_note
                                    .or_else(|| {
                                        engine
                                            .snapshot()
                                            .notes
                                            .iter()
                                            .find(|n| {
                                                !existing_notes.as_ref().unwrap().contains(&n.id)
                                            })
                                            .map(|n| n.id.clone())
                                    })
                                    .ok_or_else(|| {
                                        "保存完成，但未能定位笔记，请刷新笔记库".to_owned()
                                    }),
                                Err(error) => Err(error.clone()),
                            });
                        }
                        library_replaced = restoring && result.is_ok();
                        library_result =
                            restoring.then(|| result.as_ref().map(|_| ()).map_err(Clone::clone));
                        if library_replaced {
                            generation += 1;
                            last_sync_attempt = Instant::now();
                            sync_changed = true;
                        }
                        notice = (!matches!(&result, Ok(message) if message.is_empty()))
                            .then_some(result);
                        changed = true;
                    }
                    Ok(Work::SavePreferences(request)) => {
                        let result = preferences
                            .save(request, &mut engine, &mut synchronization)
                            .map_err(|error| format!("{error:#}"));
                        notice = Some(result.clone().map(|()| "全部设置已保存".into()));
                        preferences_result = Some(result);
                        last_sync_attempt = Instant::now();
                        sync_changed = true;
                        changed = true;
                    }
                    Ok(Work::ImportPaths(paths)) => {
                        let sender = command_sender.clone();
                        thread::spawn(move || {
                            let _ = sender.send(Work::PreparedImport {
                                generation,
                                selection: crate::imports::collect(paths),
                            });
                        });
                        notice = Some(Ok("正在检查拖入的文件和目录…".into()));
                        changed = true;
                    }
                    Ok(Work::PreparedImport {
                        generation: prepared_generation,
                        selection,
                    }) => {
                        if prepared_generation != generation {
                            continue;
                        }
                        let result = engine
                            .apply(Command::Import(selection.files), now)
                            .map_err(|e| format!("{e:#}"));
                        notice = Some(result.map(|message| {
                            if selection.warnings.is_empty() {
                                message
                            } else {
                                format!("{}；{}", message, selection.warnings.join("；"))
                            }
                        }));
                        changed = true;
                    }
                    Ok(Work::StartSync { test_only }) => {
                        if let Err(error) = launch_sync(
                            &mut synchronization,
                            &engine,
                            &command_sender,
                            generation,
                            test_only,
                        ) {
                            if !synchronization.status.busy {
                                synchronization.fail(error.to_string());
                            }
                            notice = Some(Err(error.to_string()));
                        }
                        last_sync_attempt = Instant::now();
                        sync_changed = true;
                        changed = true;
                    }
                    Ok(Work::SyncFinished {
                        generation: completed_generation,
                        result,
                    }) => {
                        if generation == completed_generation {
                            let result = synchronization
                                .finish(result, &mut engine, now)
                                .map_err(|e| e.to_string());
                            if let Err(error) = &result {
                                synchronization.fail(error.clone());
                            }
                            notice = Some(result);
                            sync_changed = true;
                            changed = true;
                        }
                    }
                    Ok(Work::SwitchLibrary(directory)) => {
                        let result = (|| -> anyhow::Result<String> {
                            anyhow::ensure!(
                                !synchronization.status.busy && !image_cloud_busy,
                                "同步正在运行，请等待完成后再切换资料库"
                            );
                            engine.checkpoint()?;
                            std::fs::create_dir_all(&directory)?;
                            let (next_lock, next, next_preferences, next_sync) =
                                open_library(&directory.join("studyapp.sqlite3"), now)?;
                            if let Some(config) = &location_config {
                                let snapshot = next.snapshot();
                                let directory = std::path::Path::new(&snapshot.data_path)
                                    .parent()
                                    .ok_or_else(|| anyhow::anyhow!("资料库目录不存在"))?;
                                crate::storage_backend::persist_location(config, directory)?;
                            }
                            engine = next;
                            preferences = next_preferences;
                            synchronization = next_sync;
                            library_lock = next_lock;
                            last_tick = Instant::now();
                            Ok("已切换资料库，原资料仍保存在原目录".into())
                        })();
                        library_replaced = result.is_ok();
                        if library_replaced {
                            generation += 1;
                            last_sync_attempt = Instant::now();
                            sync_changed = true;
                        }
                        let result = result.map_err(|e| format!("{e:#}"));
                        library_result = Some(result.as_ref().map(|_| ()).map_err(Clone::clone));
                        notice = Some(result);
                        changed = true;
                    }
                    Ok(Work::RefreshStorage) => {
                        let result = crate::storage_backend::inspect(&engine.snapshot())
                            .map_err(|error| crate::storage_backend::user_message(&error));
                        storage_event = Some(StorageEvent::Finished(result));
                        changed = true;
                    }
                    Ok(Work::DeleteManagedCopy { note_id }) => {
                        let result = if synchronization.status.busy || image_cloud_busy {
                            Err("同步正在运行，请等待完成后再删除托管副本".into())
                        } else {
                            crate::storage_backend::delete_managed_copy(&mut engine, &note_id, now)
                                .map_err(|error| crate::storage_backend::user_message(&error))
                        };
                        notice = result
                            .as_ref()
                            .ok()
                            .and_then(|report| report.cleanup.as_ref())
                            .map(crate::storage_backend::CopyCleanupOutcome::notice);
                        storage_event = Some(StorageEvent::Finished(result));
                        changed = true;
                    }
                    Ok(Work::MigrateLibrary(directory)) => {
                        let result =
                            (|| -> anyhow::Result<crate::storage_backend::StorageReport> {
                                anyhow::ensure!(
                                    !synchronization.status.busy && !image_cloud_busy,
                                    "同步正在运行，请等待完成后再迁移资料库"
                                );
                                let database = crate::storage_backend::prepare_migration(
                                    &mut engine,
                                    &directory,
                                    now,
                                )?;
                                let (next_lock, next, next_preferences, next_sync) =
                                    open_library(&database, now)?;
                                let report = crate::storage_backend::inspect(&next.snapshot())?;
                                if let Some(config) = &location_config {
                                    let directory = database
                                        .parent()
                                        .ok_or_else(|| anyhow::anyhow!("迁移目录不存在"))?;
                                    crate::storage_backend::persist_location(config, directory)?;
                                }
                                engine = next;
                                preferences = next_preferences;
                                synchronization = next_sync;
                                library_lock = next_lock;
                                last_tick = Instant::now();
                                Ok(report)
                            })()
                            .map_err(|error| crate::storage_backend::user_message(&error));
                        library_replaced = result.is_ok();
                        if library_replaced {
                            generation += 1;
                            last_sync_attempt = Instant::now();
                            sync_changed = true;
                        }
                        notice = result
                            .as_ref()
                            .ok()
                            .map(|_| Ok("资料库已迁移并验证，原目录和全部源文件仍保留".into()));
                        storage_event = Some(StorageEvent::Finished(result));
                        changed = true;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                if !synchronization.status.busy
                    && synchronization
                        .status
                        .settings
                        .auto_minutes
                        .is_some_and(|minutes| {
                            last_sync_attempt.elapsed()
                                >= Duration::from_secs(u64::from(minutes) * 60)
                        })
                    && !matches!(
                        synchronization.status.settings.target,
                        crate::sync_runtime::SyncTarget::Disabled
                    )
                {
                    if let Err(error) = launch_sync(
                        &mut synchronization,
                        &engine,
                        &command_sender,
                        generation,
                        false,
                    ) {
                        synchronization.fail(error.to_string());
                    }
                    last_sync_attempt = Instant::now();
                    sync_changed = true;
                    changed = true;
                }
                // Polling uses content hashes and the core's stability debounce.
                // All reads happen off the UI thread, including the boundary scan.
                if last_scan.elapsed() >= Duration::from_secs(3) {
                    if let Err(e) = engine.apply(Command::Scan { offline: false }, now) {
                        notice = Some(Err(format!("笔记扫描失败：{e:#}")));
                    }
                    last_scan = Instant::now();
                    changed = true;
                }
                let elapsed = last_tick.elapsed().as_secs();
                if elapsed >= 1 {
                    if let Err(e) = engine.tick(now, elapsed) {
                        notice = Some(Err(format!("计时保存失败：{e:#}")));
                    }
                    last_tick += Duration::from_secs(elapsed);
                    changed = true;
                }
                if last_checkpoint.elapsed() >= Duration::from_secs(5) {
                    if let Err(e) = engine.checkpoint() {
                        notice = Some(Err(format!("资料保存失败：{e:#}")));
                    }
                    last_checkpoint = Instant::now();
                }
                if changed {
                    let snapshot = Arc::new(engine.snapshot());
                    let read_models = read_model_cache.refresh(
                        &snapshot,
                        &chrono::Local,
                        chrono::Utc::now().timestamp(),
                    );
                    if updates
                        .send(Update {
                            startup_failure: None,
                            snapshot: Some(snapshot),
                            read_models: Some(read_models),
                            notice,
                            editor_result,
                            library_replaced,
                            library_result,
                            note_result,
                            sync_status: sync_changed.then(|| synchronization.status.clone()),
                            storage_event,
                            preferences: Some(preferences.snapshot()),
                            preferences_result,
                            media_result: None,
                            image_cloud_busy: Some(image_cloud_busy),
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }
            drop(library_lock);
        });
        Self {
            sender,
            receiver,
            handle: Some(handle),
            #[cfg(test)]
            startup_probe,
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        #[cfg(test)]
        self.startup_probe.observe("Drop", "enter");
        let _ = self.sender.send(Work::Shutdown);
        if let Some(handle) = self.handle.take() {
            #[cfg(test)]
            self.startup_probe.observe("Join", "enter");
            let joined = handle.join();
            #[cfg(test)]
            self.startup_probe
                .observe("Join", if joined.is_ok() { "exit" } else { "error" });
            let _ = joined;
        }
        #[cfg(test)]
        {
            self.startup_probe.observe("Drop", "exit");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fresh libraries durably publish defaults and preferences. Parallel Windows
    // runners can take over 10 seconds; bound readiness without weakening results.
    const INITIALIZATION_BUDGET: Duration = Duration::from_secs(60);
    const STORAGE_COMPLETION_BUDGET: Duration = Duration::from_secs(60);

    // Log failure timing before panic/Drop can spend more time joining a worker.
    fn initial_update(worker: &Worker) -> Update {
        let started = Instant::now();
        worker.startup_probe.observe("InitialReceive", "enter");
        let received = worker.receiver.recv_timeout(INITIALIZATION_BUDGET);
        worker.startup_probe.observe(
            "InitialReceive",
            &format!(
                "{} wait={}us result={:?}",
                if received.is_ok() { "exit" } else { "error" },
                started.elapsed().as_micros(),
                received
                    .as_ref()
                    .map(|update| if update.startup_failure.is_some() {
                        "startup_failure"
                    } else {
                        "snapshot"
                    })
            ),
        );
        received.expect("initial worker update exceeded 60s initialization budget")
    }

    #[test]
    fn startup_failure_keeps_worker_alive_and_allows_another_library() {
        let root = tempfile::tempdir().unwrap().keep();
        let original = root.join("original");
        std::fs::create_dir_all(original.join("settings")).unwrap();
        let manifest = original.join("settings/manifest.json");
        std::fs::write(&manifest, b"retained invalid manifest").unwrap();
        let worker = Worker::start(original.join("studyapp.sqlite3"), None);
        let failed = initial_update(&worker);
        assert!(failed.snapshot.is_none());
        assert!(failed.notice.unwrap().is_err());
        assert!(
            matches!(
                worker.receiver.recv_timeout(Duration::from_millis(50)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ),
            "a recoverable startup error must not disconnect the worker"
        );
        worker
            .sender
            .send(Work::SwitchLibrary(root.join("another")))
            .unwrap();
        let opened = worker
            .receiver
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        assert!(opened.snapshot.is_some());
        assert!(opened.library_result.unwrap().is_ok());
        assert_eq!(
            std::fs::read(&manifest).unwrap(),
            b"retained invalid manifest"
        );
    }

    #[cfg(windows)]
    #[test]
    fn startup_permission_failure_is_actionable_without_raw_os_error() {
        let root = tempfile::tempdir().unwrap().keep();
        let lock = root.join("studyapp.lock");
        std::fs::write(&lock, b"retained lock content").unwrap();
        let original_permissions = std::fs::metadata(&lock).unwrap().permissions();
        let mut permissions = original_permissions.clone();
        permissions.set_readonly(true);
        std::fs::set_permissions(&lock, permissions).unwrap();
        let worker = Worker::start(root.join("studyapp.sqlite3"), None);
        let failed = initial_update(&worker);
        let failure = failed.startup_failure.as_ref().unwrap();
        let restricted = startup::process_integrity_level().is_some_and(|level| level < 0x2000);
        assert_eq!(
            failure.kind,
            if restricted {
                startup::StartupFailureKind::RestrictedProcess
            } else {
                startup::StartupFailureKind::PermissionDenied
            }
        );
        assert_eq!(failure.stage, StartupStage::LibraryLock);
        assert_eq!(failure.path, lock);
        let message = failed.notice.unwrap().unwrap_err();
        if restricted {
            assert!(message.contains("受限环境"), "{message}");
            assert!(message.contains("构建到受限工作区之外"), "{message}");
        } else {
            assert!(message.contains("权限"), "{message}");
            assert!(!message.contains("受限环境"), "{message}");
        }
        assert!(message.contains("重试"), "{message}");
        assert!(!message.contains("os error"), "{message}");
        assert!(!root.join("studyapp.sqlite3").exists());
        assert_eq!(std::fs::read(&lock).unwrap(), b"retained lock content");
        std::fs::set_permissions(&lock, original_permissions).unwrap();
        worker.sender.send(Work::RetryOpenLibrary).unwrap();
        let recovered = worker
            .receiver
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        assert!(recovered.startup_failure.is_none());
        assert!(recovered.library_result.unwrap().is_ok());
        assert!(recovered.snapshot.is_some());
        drop(worker);
        assert_eq!(std::fs::read(&lock).unwrap(), b"retained lock content");
    }

    #[test]
    fn startup_retry_after_lock_release_reopens_original_data() {
        let root = tempfile::tempdir().unwrap().keep();
        let database = root.join("studyapp.sqlite3");
        let owner = Worker::start(database.clone(), None);
        assert!(initial_update(&owner).snapshot.is_some());
        let saved = apply(
            &owner,
            Command::CreateNote {
                title: "Existing library".into(),
                content: "Retain original content".into(),
            },
        );
        let note = saved.snapshot.unwrap().notes[0].clone();
        let waiting = Worker::start(database.clone(), None);
        let failed = initial_update(&waiting);
        let failure = failed.startup_failure.unwrap();
        assert_eq!(failure.kind, startup::StartupFailureKind::InUse);
        assert_eq!(failure.database, database);
        waiting.sender.send(Work::RetryOpenLibrary).unwrap();
        let still_busy = waiting
            .receiver
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        assert!(still_busy.snapshot.is_none());
        assert!(still_busy.library_result.unwrap().is_err());
        drop(owner);
        waiting.sender.send(Work::RetryOpenLibrary).unwrap();
        let reopened = waiting
            .receiver
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        assert!(reopened.startup_failure.is_none());
        assert!(reopened.library_result.unwrap().is_ok());
        assert!(reopened.library_replaced);
        let snapshot = reopened.snapshot.unwrap();
        assert_eq!(snapshot.notes[0].id, note.id);
        assert_eq!(snapshot.versions[0].content, "Retain original content");
    }

    #[cfg(windows)]
    #[test]
    fn startup_settings_failure_reports_exact_file_and_recovers_after_release() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = tempfile::tempdir().unwrap().keep();
        let initial = Worker::start(root.join("studyapp.sqlite3"), None);
        let snapshot = initial_update(&initial).snapshot.unwrap();
        drop(initial);
        let database = PathBuf::from(&snapshot.data_path);
        let manifest = database.parent().unwrap().join("settings/manifest.json");
        let held = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(&manifest)
            .unwrap();
        let worker = Worker::start(database, None);
        let failed = initial_update(&worker);
        let failure = failed.startup_failure.unwrap();
        assert_eq!(failure.stage, StartupStage::Preferences);
        assert_eq!(failure.kind, startup::StartupFailureKind::InUse);
        assert_eq!(failure.path, manifest);
        drop(held);
        worker.sender.send(Work::RetryOpenLibrary).unwrap();
        assert!(
            worker
                .receiver
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .snapshot
                .is_some()
        );
    }

    #[test]
    fn unified_preferences_results_and_legacy_patches_preserve_all_categories() {
        let root = tempfile::tempdir().unwrap().keep();
        let worker = Worker::start(root.join("studyapp.sqlite3"), None);
        let initial = initial_update(&worker);
        let original = initial.preferences.unwrap();
        let library = initial.snapshot.unwrap().data_path.clone();
        let mut settings = original.settings.clone();
        settings.layout.folders_visible = false;
        settings.answer_minutes = 9;
        worker
            .sender
            .send(Work::Scoped {
                library: library.clone(),
                work: Box::new(Work::SavePreferences(SavePreferences {
                    expected_revision: original.revision.clone(),
                    settings,
                    shortcuts: original.shortcuts.clone(),
                    sync: original.sync.clone(),
                    password: None,
                    image_secret: Default::default(),
                })),
            })
            .unwrap();
        let saved = receive_notice(&worker);
        assert!(saved.preferences_result.unwrap().is_ok());
        let committed = saved.preferences.unwrap();
        assert_eq!(committed.settings.answer_minutes, 9);
        assert!(!committed.settings.layout.folders_visible);
        let patched = apply(&worker, Command::SetShowFilePaths(true));
        let patched = patched.preferences.unwrap();
        assert!(patched.settings.show_file_paths);
        assert_eq!(patched.settings.answer_minutes, 9);
        assert!(!patched.settings.layout.folders_visible);
        assert_eq!(patched.shortcuts, committed.shortcuts);
        worker
            .sender
            .send(Work::SavePreferences(SavePreferences {
                expected_revision: original.revision,
                settings: original.settings,
                shortcuts: original.shortcuts,
                sync: original.sync,
                password: None,
                image_secret: Default::default(),
            }))
            .unwrap();
        let rejected = receive_notice(&worker);
        assert!(rejected.preferences_result.unwrap().is_err());
        assert_eq!(rejected.preferences.unwrap().revision, patched.revision);
    }

    fn storage_result(update: &Update) -> &Result<crate::storage_backend::StorageReport, String> {
        let Some(StorageEvent::Finished(result)) = &update.storage_event else {
            panic!("storage request must complete");
        };
        result
    }

    #[test]
    fn retained_draft_commit_reports_the_provided_note_id_and_collision() {
        let root = tempfile::tempdir().unwrap().keep();
        let worker = Worker::start(root.join("studyapp.sqlite3"), None);
        initial_update(&worker);
        let note_id = "36b79f08-5128-43ab-a1cb-13f318c455f7";
        let command = Command::CreateNoteWithId {
            note_id: note_id.into(),
            folder_id: None,
            title: "draft".into(),
            content: "original".into(),
        };
        let created = apply(&worker, command.clone());
        assert_eq!(created.note_result.unwrap().unwrap(), note_id);
        let rejected = apply(&worker, command);
        assert!(rejected.note_result.unwrap().is_err());
        assert_eq!(rejected.snapshot.unwrap().notes.len(), 1);
    }

    #[test]
    fn storage_refresh_delete_and_migration_complete_and_remember_restart_location() {
        let root = tempfile::tempdir().unwrap().keep();
        let source = root.join("original");
        let config = root.join("config/location.json");
        let worker = Worker::start(source.join("studyapp.sqlite3"), Some(config.clone()));
        initial_update(&worker);
        let created = apply(
            &worker,
            Command::CreateNoteInFolder {
                title: "storage fixture".into(),
                content: "retained".into(),
                folder_id: None,
            },
        );
        assert!(created.note_result.as_ref().unwrap().is_ok());
        let note = created.snapshot.unwrap().notes[0].clone();
        worker.sender.send(Work::RefreshStorage).unwrap();
        let report = receive_notice(&worker);
        assert_eq!(storage_result(&report).as_ref().unwrap().managed_bytes, 8);
        std::fs::write(source.join("note-draft.json"), "null").unwrap();
        let target = root.join("migrated");
        worker
            .sender
            .send(Work::MigrateLibrary(target.clone()))
            .unwrap();
        let migrated = receive_notice(&worker);
        assert!(storage_result(&migrated).is_ok(), "{:?}", migrated.notice);
        assert!(migrated.library_replaced);
        let migrated_note = migrated.snapshot.unwrap().notes[0].clone();
        assert_eq!(migrated_note.id, note.id);
        assert_ne!(migrated_note.path, note.path);
        assert_eq!(std::fs::read_to_string(&note.path).unwrap(), "retained");
        assert_eq!(
            std::fs::read_to_string(target.join("note-draft.json")).unwrap(),
            "null"
        );
        let remembered: PathBuf = serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
        assert_eq!(
            std::fs::canonicalize(&remembered).unwrap(),
            std::fs::canonicalize(&target).unwrap()
        );
        worker
            .sender
            .send(Work::DeleteManagedCopy {
                note_id: note.id.clone(),
            })
            .unwrap();
        let deleted = receive_notice(&worker);
        assert!(storage_result(&deleted).is_ok(), "{:?}", deleted.notice);
        assert!(deleted.snapshot.unwrap().notes[0].detached);
        assert!(!PathBuf::from(migrated_note.path).exists());
        assert_eq!(std::fs::read_to_string(&note.path).unwrap(), "retained");
        drop(worker);
        let restarted = Worker::start(remembered.join("studyapp.sqlite3"), Some(config));
        let restarted = initial_update(&restarted);
        assert_eq!(restarted.snapshot.as_ref().unwrap().notes[0].id, note.id);
        assert!(restarted.snapshot.unwrap().notes[0].detached);
    }

    #[test]
    fn migration_location_publish_failure_keeps_original_library_and_configuration() {
        let root = tempfile::tempdir().unwrap().keep();
        let config = root.join("location.json");
        std::fs::create_dir(&config).unwrap();
        let worker = Worker::start(root.join("original/studyapp.sqlite3"), Some(config.clone()));
        let origin = initial_update(&worker).snapshot.unwrap().data_path.clone();
        let created = apply(
            &worker,
            Command::CreateNote {
                title: "preserve".into(),
                content: "original".into(),
            },
        );
        let note = created.snapshot.unwrap().notes[0].clone();
        worker
            .sender
            .send(Work::MigrateLibrary(root.join("target")))
            .unwrap();
        let rejected = receive_notice(&worker);
        assert!(storage_result(&rejected).is_err());
        assert!(!rejected.library_replaced);
        assert_eq!(rejected.snapshot.unwrap().data_path, origin);
        assert!(config.is_dir());
        assert_eq!(std::fs::read_to_string(note.path).unwrap(), "original");
    }

    #[test]
    fn explicit_data_directory_migration_does_not_write_global_location() {
        let root = tempfile::tempdir().unwrap().keep();
        let unrelated_global = root.join("global/location.json");
        std::fs::create_dir(unrelated_global.parent().unwrap()).unwrap();
        std::fs::write(&unrelated_global, b"unchanged global preference").unwrap();
        let worker = Worker::start(root.join("isolated/studyapp.sqlite3"), None);
        initial_update(&worker);
        worker
            .sender
            .send(Work::MigrateLibrary(root.join("next-isolated")))
            .unwrap();
        let result = receive_notice(&worker);
        assert!(storage_result(&result).is_ok(), "{:?}", result.notice);
        assert!(result.library_replaced);
        assert_eq!(
            std::fs::read(unrelated_global).unwrap(),
            b"unchanged global preference"
        );
        assert!(!root.join("location.json").exists());
    }

    #[test]
    fn storage_mutations_are_rejected_during_sync_and_stale_requests_always_complete() {
        let root = tempfile::tempdir().unwrap().keep();
        let worker = Worker::start(root.join("original/studyapp.sqlite3"), None);
        let origin = initial_update(&worker).snapshot.unwrap().data_path.clone();
        let created = apply(
            &worker,
            Command::CreateNote {
                title: "preserve".into(),
                content: "original".into(),
            },
        );
        let note = created.snapshot.unwrap().notes[0].clone();
        let rejected = while_network_sync_is_busy(&worker, || {
            worker
                .sender
                .send(Work::DeleteManagedCopy {
                    note_id: note.id.clone(),
                })
                .unwrap();
            let deleted = receive_notice(&worker);
            assert!(
                storage_result(&deleted)
                    .as_ref()
                    .unwrap_err()
                    .contains("同步")
            );
            assert!(!deleted.snapshot.unwrap().notes[0].detached);
            worker
                .sender
                .send(Work::MigrateLibrary(root.join("busy-target")))
                .unwrap();
            receive_notice(&worker)
        });
        assert!(
            storage_result(&rejected)
                .as_ref()
                .unwrap_err()
                .contains("同步")
        );
        assert!(!root.join("busy-target").exists());
        receive_notice(&worker);
        worker
            .sender
            .send(Work::SwitchLibrary(root.join("next")))
            .unwrap();
        assert!(receive_notice(&worker).library_replaced);
        for work in [
            Work::RefreshStorage,
            Work::DeleteManagedCopy {
                note_id: note.id.clone(),
            },
            Work::MigrateLibrary(root.join("stale-target")),
        ] {
            worker
                .sender
                .send(Work::Scoped {
                    library: origin.clone(),
                    work: Box::new(work),
                })
                .unwrap();
            let rejected = receive_notice(&worker);
            assert!(storage_result(&rejected).is_err());
            assert!(rejected.notice.unwrap().is_err());
            assert!(rejected.note_result.is_none());
            assert!(!rejected.editor_result);
        }
        assert_eq!(std::fs::read_to_string(note.path).unwrap(), "original");
    }

    #[test]
    fn unmanage_is_rejected_while_network_sync_is_busy() {
        let root = tempfile::tempdir().unwrap().keep();
        let worker = Worker::start(root.join("study.sqlite3"), None);
        initial_update(&worker);
        let created = apply(
            &worker,
            Command::CreateNote {
                title: "protected during sync".into(),
                content: "retained".into(),
            },
        );
        let id = created.snapshot.unwrap().notes[0].id.clone();
        let result = while_network_sync_is_busy(&worker, || {
            apply(&worker, Command::UnmanageNote { note_id: id })
        });
        assert!(result.notice.unwrap().unwrap_err().contains("同步"));
        assert!(!result.snapshot.unwrap().notes[0].detached);
    }

    #[test]
    fn restore_is_rejected_while_sync_is_busy_and_succeeds_after_it_finishes() {
        let root = tempfile::tempdir().unwrap().keep();
        let worker = Worker::start(root.join("study.sqlite3"), None);
        initial_update(&worker);
        let created = apply(
            &worker,
            Command::CreateNote {
                title: "restore after sync".into(),
                content: "retained".into(),
            },
        );
        let note = created.snapshot.unwrap().notes[0].clone();
        apply(
            &worker,
            Command::UnmanageNote {
                note_id: note.id.clone(),
            },
        )
        .notice
        .unwrap()
        .unwrap();
        let backup = root.join("detached.sqlite3");
        apply(&worker, Command::Backup(backup.clone()))
            .notice
            .unwrap()
            .unwrap();
        let reimported = apply(&worker, Command::Import(vec![PathBuf::from(&note.path)]));
        reimported.notice.unwrap().unwrap();
        assert!(!reimported.snapshot.unwrap().notes[0].detached);

        let rejected = while_network_sync_is_busy(&worker, || {
            apply(&worker, Command::Restore(backup.clone()))
        });
        assert!(rejected.notice.unwrap().unwrap_err().contains("同步"));
        assert!(!rejected.library_replaced);
        assert!(!rejected.snapshot.unwrap().notes[0].detached);
        assert_eq!(std::fs::read_to_string(&note.path).unwrap(), "retained");

        let finished = receive_notice(&worker);
        assert!(!finished.sync_status.unwrap().busy);
        let restored = apply(&worker, Command::Restore(backup));
        restored.notice.unwrap().unwrap();
        assert!(restored.library_replaced);
        assert!(restored.snapshot.unwrap().notes[0].detached);
    }

    #[test]
    fn library_switch_is_rejected_while_sync_is_busy_and_succeeds_after_it_finishes() {
        let root = tempfile::tempdir().unwrap().keep();
        let worker = Worker::start(root.join("study.sqlite3"), None);
        let origin = initial_update(&worker).snapshot.unwrap().data_path.clone();
        let next = root.join("next");
        let rejected = while_network_sync_is_busy(&worker, || {
            worker
                .sender
                .send(Work::SwitchLibrary(next.clone()))
                .unwrap();
            receive_notice(&worker)
        });
        assert!(rejected.notice.unwrap().unwrap_err().contains("同步"));
        assert!(!rejected.library_replaced);
        assert_eq!(rejected.snapshot.unwrap().data_path, origin);
        assert!(!next.exists());

        let finished = receive_notice(&worker);
        assert!(!finished.sync_status.unwrap().busy);
        worker
            .sender
            .send(Work::SwitchLibrary(next.clone()))
            .unwrap();
        let switched = receive_notice(&worker);
        switched.notice.unwrap().unwrap();
        assert!(switched.library_replaced);
        assert_ne!(switched.snapshot.unwrap().data_path, origin);
        assert!(next.join("studyapp.sqlite3").is_file());
    }

    fn while_network_sync_is_busy(worker: &Worker, action: impl FnOnce() -> Update) -> Update {
        use std::io::{Read, Write};
        worker.sender.send(Work::RefreshStorage).unwrap();
        let current = receive_notice(worker).preferences.unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        // Hold the initial TLS handshake so the worker has a deterministic,
        // in-flight network task. Release with invalid TLS after the assertion.
        let url = format!("https://{}/", listener.local_addr().unwrap());
        let (entered, wait_entered) = mpsc::channel();
        let (release, wait_release) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut request = [0; 4096];
            assert!(stream.read(&mut request).unwrap() > 0);
            entered.send(()).unwrap();
            wait_release.recv_timeout(Duration::from_secs(10)).unwrap();
            stream
                .write_all(b"HTTP/1.1 500 Failed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .unwrap();
        });
        worker
            .sender
            .send(Work::SavePreferences(SavePreferences {
                expected_revision: current.revision,
                settings: current.settings,
                shortcuts: current.shortcuts,
                sync: crate::sync_runtime::SyncSettings {
                    target: crate::sync_runtime::SyncTarget::WebDav {
                        url,
                        username: "fixture".into(),
                    },
                    auto_minutes: None,
                },
                password: Some(zeroize::Zeroizing::new("fixture".into())),
                image_secret: Default::default(),
            }))
            .unwrap();
        receive_notice(worker).notice.unwrap().unwrap();
        worker
            .sender
            .send(Work::StartSync { test_only: false })
            .unwrap();
        wait_entered.recv_timeout(Duration::from_secs(10)).unwrap();
        let result = action();
        release.send(()).unwrap();
        server.join().unwrap();
        result
    }

    #[test]
    fn stale_ui_commands_and_import_completions_cannot_cross_libraries() {
        let root = tempfile::tempdir().unwrap().keep();
        let worker = Worker::start(root.join("original.sqlite3"), None);
        let origin = initial_update(&worker).snapshot.unwrap().data_path.clone();
        worker
            .sender
            .send(Work::SwitchLibrary(root.join("next")))
            .unwrap();
        assert!(receive_notice(&worker).library_replaced);
        worker
            .sender
            .send(Work::Scoped {
                library: origin,
                work: Box::new(Work::Apply(Command::CreateNote {
                    title: "旧库草稿".into(),
                    content: "不可写入新库".into(),
                })),
            })
            .unwrap();
        let rejected = receive_notice(&worker);
        assert!(rejected.notice.unwrap().is_err());
        assert!(rejected.note_result.unwrap().is_err());
        assert!(rejected.snapshot.unwrap().notes.is_empty());
        let source = root.join("late.md");
        std::fs::write(&source, "late import").unwrap();
        worker
            .sender
            .send(Work::PreparedImport {
                generation: 0,
                selection: crate::imports::collect(vec![source]),
            })
            .unwrap();
        let barrier = apply(&worker, Command::SetPaused(true));
        assert!(barrier.snapshot.unwrap().notes.is_empty());
    }

    #[test]
    fn stale_library_actions_complete_with_their_own_failure_signal() {
        let root = tempfile::tempdir().unwrap().keep();
        let worker = Worker::start(root.join("studyapp.sqlite3"), None);
        let initial = initial_update(&worker);
        assert!(initial.library_result.is_none());
        let original = initial.snapshot.unwrap().data_path.clone();
        for work in [
            Work::SwitchLibrary(root.join("must-not-open")),
            Work::Apply(Command::Restore(root.join("missing.sqlite3"))),
        ] {
            worker
                .sender
                .send(Work::Scoped {
                    library: "expired-library".into(),
                    work: Box::new(work),
                })
                .unwrap();
            let rejected = receive_notice(&worker);
            assert!(rejected.library_result.as_ref().unwrap().is_err());
            assert!(!rejected.library_replaced);
            assert!(rejected.preferences_result.is_none());
            assert_eq!(rejected.snapshot.unwrap().data_path, original);
        }
        assert!(!root.join("must-not-open").exists());
    }

    #[test]
    fn stale_commands_report_only_their_own_editor_result() {
        let root = tempfile::tempdir().unwrap().keep();
        let worker = Worker::start(root.join("original.sqlite3"), None);
        let origin = initial_update(&worker).snapshot.unwrap().data_path.clone();
        worker
            .sender
            .send(Work::SwitchLibrary(root.join("next")))
            .unwrap();
        assert!(receive_notice(&worker).library_replaced);
        for (work, note_result, editor_result) in [
            (
                Work::Apply(Command::ExportNote {
                    note_id: "old-note".into(),
                    version_id: None,
                    path: root.join("must-not-export.md"),
                }),
                false,
                false,
            ),
            (
                Work::Apply(Command::Backup(root.join("must-not-backup.sqlite3"))),
                false,
                false,
            ),
            (Work::ImportPaths(vec![]), false, false),
            (
                Work::Apply(Command::CreateNote {
                    title: "stale new note".into(),
                    content: "preserve UI draft".into(),
                }),
                true,
                false,
            ),
            (
                Work::Apply(Command::CreateNoteInFolder {
                    title: "stale folder note".into(),
                    content: "preserve folder draft".into(),
                    folder_id: None,
                }),
                true,
                false,
            ),
            (
                Work::Apply(Command::SaveNote {
                    note_id: "old-note".into(),
                    expected_hash: "old-hash".into(),
                    content: "preserve UI draft".into(),
                }),
                true,
                false,
            ),
            (
                Work::Apply(Command::Manual {
                    started_at: 1000,
                    minutes: 1,
                    summary: "stale session".into(),
                    note_ids: vec![],
                    version_ids: None,
                }),
                false,
                true,
            ),
            (
                Work::Apply(Command::Submit {
                    session_id: "old-session".into(),
                    summary: "stale pending session".into(),
                    note_ids: vec![],
                    version_ids: None,
                }),
                false,
                true,
            ),
            (
                Work::Apply(Command::EditSession {
                    session_id: "old-session".into(),
                    summary: "stale session edit".into(),
                    learned: true,
                    note_ids: vec![],
                    version_ids: None,
                }),
                false,
                true,
            ),
        ] {
            worker
                .sender
                .send(Work::Scoped {
                    library: origin.clone(),
                    work: Box::new(work),
                })
                .unwrap();
            let rejected = receive_notice(&worker);
            assert!(rejected.notice.unwrap().is_err());
            assert_eq!(rejected.note_result.is_some(), note_result);
            assert!(rejected.note_result.is_none_or(|result| result.is_err()));
            assert_eq!(rejected.editor_result, editor_result);
            assert!(rejected.snapshot.unwrap().notes.is_empty());
        }
        assert!(!root.join("must-not-export.md").exists());
        assert!(!root.join("must-not-backup.sqlite3").exists());
    }

    fn receive_notice(worker: &Worker) -> Update {
        // Storage completion is intentionally silent; notifications are not
        // the operation protocol. Bound the whole wait, including periodic ticks.
        let deadline = Instant::now() + STORAGE_COMPLETION_BUDGET;
        loop {
            let update = worker
                .receiver
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap();
            if update.notice.is_some() || update.storage_event.is_some() {
                return update;
            }
            assert!(!update.library_replaced);
        }
    }

    fn apply(worker: &Worker, command: Command) -> Update {
        worker.sender.send(Work::Apply(command)).unwrap();
        receive_notice(worker)
    }

    #[test]
    fn restore_same_library_marks_success_and_preserves_data_on_failure() {
        let directory = tempfile::tempdir().unwrap().keep();
        let worker = Worker::start(directory.join("study.sqlite3"), None);
        let initial = initial_update(&worker);
        assert!(initial.snapshot.is_some());
        assert!(!initial.library_replaced);
        let settings = studyapp_core::Settings {
            interval_minutes: 5,
            ..Default::default()
        };
        let updated = apply(&worker, Command::SaveSettings(settings));
        assert!(updated.notice.unwrap().is_ok());
        assert!(!updated.library_replaced);
        let now = chrono::Utc::now().timestamp();
        let saved = apply(
            &worker,
            Command::Manual {
                version_ids: None,
                started_at: now - 120,
                minutes: 1,
                summary: "备份中的学习".into(),
                note_ids: vec![],
            },
        );
        assert!(saved.notice.unwrap().is_ok());
        assert!(!saved.library_replaced);
        let original_id = saved.snapshot.unwrap().sessions[0].id.clone();
        let backup = directory.join("saved.sqlite3");
        let backed_up = apply(&worker, Command::Backup(backup.clone()));
        assert!(backed_up.notice.unwrap().is_ok());
        assert!(!backed_up.library_replaced);
        let updated = apply(
            &worker,
            Command::SaveSettings(studyapp_core::Settings {
                interval_minutes: 10,
                ..Default::default()
            }),
        );
        assert!(updated.notice.unwrap().is_ok());
        assert!(!updated.library_replaced);
        let extra = apply(
            &worker,
            Command::Manual {
                version_ids: None,
                started_at: now - 240,
                minutes: 1,
                summary: "备份以后的学习".into(),
                note_ids: vec![],
            },
        );
        assert_eq!(extra.snapshot.unwrap().sessions.len(), 2);
        assert!(!extra.library_replaced);
        let restored = apply(&worker, Command::Restore(backup));
        assert!(restored.notice.unwrap().is_ok());
        assert_eq!(restored.library_result, Some(Ok(())));
        assert!(restored.preferences_result.is_none());
        assert!(restored.library_replaced);
        let snapshot = restored.snapshot.unwrap();
        assert_eq!(snapshot.settings.interval_minutes, 10);
        assert_eq!(snapshot.sessions.len(), 1);
        assert_eq!(snapshot.sessions[0].id, original_id);
        assert_eq!(snapshot.sessions[0].summary, "备份中的学习");
        let failed = apply(&worker, Command::Restore(directory.join("missing.sqlite3")));
        assert!(failed.notice.unwrap().is_err());
        assert!(failed.library_result.as_ref().unwrap().is_err());
        assert!(failed.preferences_result.is_none());
        assert!(!failed.library_replaced);
        let unchanged = failed.snapshot.unwrap();
        assert_eq!(unchanged.data_path, snapshot.data_path);
        assert_eq!(unchanged.settings.interval_minutes, 10);
        assert_eq!(unchanged.sessions.len(), 1);
        assert_eq!(unchanged.sessions[0].id, original_id);
    }

    #[test]
    fn library_switch_marks_only_successful_replacements() {
        let directory = tempfile::tempdir().unwrap().keep();
        let worker = Worker::start(directory.join("original.sqlite3"), None);
        let initial = initial_update(&worker);
        assert!(!initial.library_replaced);
        assert!(initial.library_result.is_none());
        let original_path = initial.snapshot.unwrap().data_path.clone();
        let invalid_directory = directory.join("file.txt");
        std::fs::write(&invalid_directory, "preserve").unwrap();
        worker
            .sender
            .send(Work::SwitchLibrary(invalid_directory))
            .unwrap();
        let failed = receive_notice(&worker);
        assert!(failed.notice.unwrap().is_err());
        assert!(failed.library_result.as_ref().unwrap().is_err());
        assert!(failed.preferences_result.is_none());
        assert!(!failed.library_replaced);
        assert_eq!(failed.snapshot.unwrap().data_path, original_path);
        worker
            .sender
            .send(Work::SwitchLibrary(directory.join("next")))
            .unwrap();
        let switched = receive_notice(&worker);
        assert!(switched.notice.unwrap().is_ok());
        assert_eq!(switched.library_result, Some(Ok(())));
        assert!(switched.preferences_result.is_none());
        assert!(switched.library_replaced);
        assert_ne!(switched.snapshot.unwrap().data_path, original_path);
    }

    #[test]
    fn worker_reports_validation_errors_and_persists_before_shutdown() {
        let directory = tempfile::tempdir().unwrap().keep();
        let path = directory.join("study.sqlite3");
        let worker = Worker::start(path.clone(), None);
        assert!(initial_update(&worker).snapshot.is_some());
        let invalid = apply(
            &worker,
            Command::Manual {
                version_ids: None,
                started_at: chrono::Utc::now().timestamp() - 600,
                minutes: 0,
                summary: "无效时长".into(),
                note_ids: vec![],
            },
        );
        assert!(invalid.notice.unwrap().is_err());
        assert!(invalid.editor_result);
        assert!(!invalid.library_replaced);
        assert!(invalid.snapshot.unwrap().sessions.is_empty());
        worker
            .sender
            .send(Work::Apply(Command::Manual {
                version_ids: None,
                started_at: chrono::Utc::now().timestamp() - 600,
                minutes: 10,
                summary: "复习所有权".into(),
                note_ids: vec![],
            }))
            .unwrap();
        loop {
            let result = worker
                .receiver
                .recv_timeout(Duration::from_secs(10))
                .unwrap();
            if let Some(notice) = result.notice {
                assert!(notice.is_ok());
                break;
            }
        }
        drop(worker);
        let engine = Engine::open(&path, chrono::Utc::now().timestamp()).unwrap();
        assert_eq!(engine.snapshot().sessions.len(), 1);
    }

    #[test]
    fn a_second_worker_cannot_overwrite_an_open_library() {
        let directory = tempfile::tempdir().unwrap().keep();
        let path = directory.join("study.sqlite3");
        let first = Worker::start(path.clone(), None);
        assert!(initial_update(&first).snapshot.is_some());
        let second = Worker::start(path, None);
        let result = initial_update(&second);
        assert!(result.snapshot.is_none());
        assert!(!result.library_replaced);
        assert!(result.notice.unwrap().unwrap_err().contains("另一个"));
    }
}
