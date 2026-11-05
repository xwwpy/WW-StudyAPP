//! Virtual GPUI windows only: the app render/native editor is never mounted.
use super::*;
use crate::app::{Page, StudyApp};
use gpui_kit::{
    AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, TestAppContext,
    VisualTestContext, Window, div, test::TestWindowExt,
};
struct Fixture {
    app: Entity<StudyApp>,
}
impl Render for Fixture {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.app.update(cx, |app, cx| {
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(app.owner_controls(cx))
                .children(app.settings_modal(cx))
        })
    }
}
fn fixture(cx: &mut TestAppContext) -> (Entity<StudyApp>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::apply(&Default::default(), false, cx);
    });
    let root = tempfile::tempdir().unwrap().keep();
    let worker = crate::worker::Worker::start(root.join("studyapp.sqlite3"), None);
    let initial = worker
        .receiver
        .recv_timeout(Duration::from_secs(60))
        .unwrap();
    let snapshot = initial.snapshot.unwrap();
    let preferences = initial.preferences.unwrap();
    let mut app = None;
    let (_, visual) = cx.add_window_view(|window, cx| {
        let entity = cx.new(|cx| StudyApp::new(worker, window, cx));
        entity.update(cx, |app, cx| {
            app.last_library = snapshot.data_path.clone();
            app.snapshot = Some(snapshot);
            app.startup_busy = false;
            app.preferences_ui.saved = Some(preferences);
            app.page = Page::Settings;
            app.begin_settings(window, cx);
        });
        app = Some(entity.clone());
        Fixture { app: entity }
    });
    (app.unwrap(), visual)
}
fn control(database: impl Into<PathBuf>) -> Arc<Control> {
    Arc::new(Control {
        database: database.into(),
        state: Mutex::new((Phase::Requested, Some("a".repeat(32)))),
    })
}
#[gpui_kit::test]
fn owner_request_preserves_unsaved_settings_and_cancel_allows_a_new_request(
    cx: &mut TestAppContext,
) {
    let (app, visual) = fixture(cx);
    visual.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.interval
                .update(cx, |input, cx| input.set_value("31", window, cx));
            let request = control(app.last_library.clone());
            app.accept_owner_close(request.clone(), window, cx);
            app.update_owner_close();
            assert_eq!(request.phase(), Phase::Waiting);
            assert!(app.preferences_ui.leave.is_some());
            assert!(!app.close_requested);
            assert!(!app.close_allowed);
            assert!(app.settings_dirty(cx));
            assert!(!app.note_content.read(cx).creating());
        })
    });
    visual.update(|window, cx| {
        window.draw(cx).clear(cx);
        window.click("settings-continue-editing", cx);
    });
    visual.update(|window, cx| {
        app.update(cx, |app, cx| {
            let request = app.owner_close.clone().unwrap();
            app.update_owner_close();
            assert_eq!(request.phase(), Phase::Cancelled);
            assert!(app.owner_close.is_none());
            assert!(app.settings_dirty(cx));
            assert_eq!(app.interval.read(cx).value(), "31");
            let retry = control(app.last_library.clone());
            app.accept_owner_close(retry.clone(), window, cx);
            assert_eq!(retry.phase(), Phase::Waiting);
            assert!(app.preferences_ui.leave.is_some());
        })
    });
}
#[gpui_kit::test]
fn wrong_library_or_storage_operation_never_starts_normal_close(cx: &mut TestAppContext) {
    let (app, visual) = fixture(cx);
    visual.update(|window, cx| {
        app.update(cx, |app, cx| {
            let other = control(PathBuf::from("another-library.sqlite3"));
            app.accept_owner_close(other.clone(), window, cx);
            assert_eq!(other.phase(), Phase::Busy);
            assert!(!app.close_requested);
            let same = control(app.last_library.clone());
            app.storage_busy = true;
            app.accept_owner_close(same.clone(), window, cx);
            assert_eq!(same.phase(), Phase::Busy);
            assert!(app.owner_close.is_none());
            assert!(!app.close_allowed);
            app.storage_busy = false;
            let saving_settings = control(app.last_library.clone());
            app.preferences_ui.busy = true;
            app.accept_owner_close(saving_settings.clone(), window, cx);
            assert_eq!(saving_settings.phase(), Phase::Busy);
            assert!(!app.close_requested);
            app.preferences_ui.busy = false;
        })
    });
}
#[gpui_kit::test]
fn owner_close_bypasses_widget_hide_but_waits_for_durable_close_barriers(cx: &mut TestAppContext) {
    let (app, visual) = fixture(cx);
    visual.update(|window, cx| {
        app.update(cx, |app, cx| {
            let request = control(app.last_library.clone());
            app.timer_widget = Some(app.main_window);
            app.note_saving = true;
            app.accept_owner_close(request.clone(), window, cx);
            assert!(
                app.close_requested,
                "owner close must not become timer-widget minimize"
            );
            assert!(!app.close_allowed);
            assert!(app.draft_close_drain.is_none());
            assert_eq!(request.phase(), Phase::Waiting);
            assert!(is_held(&locked_file(Path::new(&app.last_library)).unwrap()).unwrap());
            app.timer_widget = None;
            app.note_saving = false;
            app.continue_close(window, cx);
            assert!(
                app.draft_close_drain.is_some(),
                "no-editor close still drains background drafts"
            );
            assert!(
                !app.close_allowed,
                "drain must be acknowledged before window removal"
            );
            // An error/cancelled continuation must be reported, never treated as exit.
            app.close_requested = false;
            app.update_owner_close();
            assert_eq!(request.phase(), Phase::Cancelled);
            assert!(is_held(&locked_file(Path::new(&app.last_library)).unwrap()).unwrap());
        })
    });
}
#[gpui_kit::test]
fn verified_owner_review_and_stop_waiting_do_not_send_a_close_request(cx: &mut TestAppContext) {
    let (app, visual) = fixture(cx);
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("studyapp.sqlite3");
    let file = lock_library(&database);
    let (tx, rx) = std::sync::mpsc::channel();
    let _server = Server::start(&database, &file, move |request| {
        tx.send(request).unwrap();
    })
    .unwrap();
    let offer = local_offer(&database, &file);
    visual.update(|_, cx| {
        app.update(cx, |app, _| {
            let mut failure = crate::worker::startup::StartupFailure::in_use(
                &database,
                &database.with_extension("lock"),
            );
            failure.owner = Some(Arc::new(offer));
            app.startup_failure = Some(failure);
        })
    });
    visual.update(|window, cx| {
        window.draw(cx).clear(cx);
        window.click("startup-close-owner", cx);
    });
    app.read_with(visual, |app, _| assert!(app.owner_recovery.review));
    assert!(rx.try_recv().is_err());
    visual.update(|window, cx| {
        window.draw(cx).clear(cx);
        window.click("owner-cancel-review", cx);
    });
    visual.update(|_, cx| {
        app.update(cx, |app, _| {
            assert!(!app.owner_recovery.review);
            app.startup_busy = true;
            app.owner_recovery.waiting = true;
        })
    });
    visual.update(|window, cx| {
        window.draw(cx).clear(cx);
        window.click("owner-stop-waiting", cx);
    });
    app.read_with(visual, |app, _| {
        assert!(!app.owner_recovery.waiting);
        assert!(!app.startup_busy);
    });
    assert!(rx.try_recv().is_err());
    assert!(is_held(&locked_file(&database).unwrap()).unwrap());
}
#[gpui_kit::test]
fn failed_draft_drain_cancels_owner_close_and_keeps_data_recoverable(cx: &mut TestAppContext) {
    use crate::drafts::{DraftRecord, DraftStore, NoteDraft};
    let (app, visual) = fixture(cx);
    let record = DraftRecord::new(
        NoteDraft {
            note_id: None,
            expected_hash: None,
            folder_id: None,
            asset_group: String::new(),
            title: "Retained draft".into(),
            content: "Must survive failed owner close".into(),
        },
        1,
    );
    let (request, path) = visual.update(|window, cx| {
        app.update(cx, |app, cx| {
            let path = DraftStore::record_path(&app.last_library, &record.id).unwrap();
            std::fs::create_dir_all(&path).unwrap(); // Not a file: real persistence failure.
            app.drafts
                .persist_record(&app.last_library, record.clone(), 191);
            let request = control(app.last_library.clone());
            app.accept_owner_close(request.clone(), window, cx);
            assert!(app.draft_close_drain.is_some());
            (request, path)
        })
    });
    let started = Instant::now();
    while request.phase() == Phase::Waiting {
        visual.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.poll_drafts(window, cx);
                app.update_owner_close();
            })
        });
        assert!(started.elapsed() < Duration::from_secs(5));
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(request.phase(), Phase::Cancelled);
    app.read_with(visual, |app, _| {
        assert!(!app.close_allowed);
        assert!(!app.close_requested);
        assert!(is_held(&locked_file(Path::new(&app.last_library)).unwrap()).unwrap());
    });
    std::fs::rename(&path, path.with_extension("retained-blocker")).unwrap();
    visual.update(|_, cx| {
        app.update(cx, |app, _| {
            app.drafts.drain(&app.last_library, 192);
            loop {
                if let crate::drafts::DraftUpdate::Drained {
                    token: 192, result, ..
                } = app
                    .drafts
                    .receiver
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap()
                {
                    result.unwrap();
                    break;
                }
            }
        })
    });
    let recovered: DraftRecord = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(recovered, record);
}
#[gpui_kit::test]
fn completed_close_releases_library_even_if_the_app_entity_is_retained(cx: &mut TestAppContext) {
    let (app, visual) = fixture(cx);
    let database = app.read_with(visual, |app, _| PathBuf::from(&app.last_library));
    let request = control(database.clone());
    visual.update(|window, cx| {
        app.update(cx, |app, cx| {
            // Represents entry from the already acknowledged durable-close barrier.
            app.owner_close = Some(request.clone());
            app.complete_close(window, cx);
        })
    });
    assert_eq!(request.phase(), Phase::Closing);
    assert!(!is_held(&locked_file(&database).unwrap()).unwrap());
    let reopened = crate::worker::Worker::start(database.clone(), None);
    assert!(
        reopened
            .receiver
            .recv_timeout(Duration::from_secs(60))
            .unwrap()
            .snapshot
            .is_some()
    );
    // This strong reference deliberately outlives the removed GPUI window.
    drop(app);
    assert!(is_held(&locked_file(&database).unwrap()).unwrap());
}
