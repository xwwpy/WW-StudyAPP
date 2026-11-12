//! User-invoked real operations; destructive operations keep their own confirms.
use crate::{app::StudyApp, controls::Button};
use gpui_kit::{
    component::{Disableable, IconName, Sizable, button::ButtonVariants},
    *,
};
use studyapp_core::NoticeStyle;
impl StudyApp {
    pub(crate) fn library_action_sheet(&self, cx: &Context<Self>) -> AnyElement {
        // The sheet blocks background controls, not its own operations.
        let busy = self.library_tasks_busy();
        let actions = vec![
            Button::new("sheet-new-note")
                .icon(IconName::FileText)
                .label("新建 Markdown")
                .disabled(busy)
                .on_click(cx.listener(|this, _, window, cx| {
                    if this.library_tasks_busy() {
                        return;
                    }
                    this.action_sheet_open = false;
                    this.new_note(window, cx);
                })),
            Button::new("sheet-new-folder")
                .icon(IconName::Folder)
                .label("新建文件夹")
                .disabled(busy || !self.folders.read(cx).can_create_folder())
                .on_click(cx.listener(|this, _, window, cx| {
                    if this.library_tasks_busy() {
                        return;
                    }
                    this.action_sheet_open = false;
                    this.folders
                        .update(cx, |panel, cx| panel.begin_create_folder(window, cx));
                    cx.notify();
                })),
            Button::new("sheet-import-file")
                .icon(IconName::FileText)
                .label("导入文件…")
                .disabled(busy)
                .on_click(cx.listener(|this, _, _, cx| {
                    if this.library_tasks_busy() {
                        return;
                    }
                    this.action_sheet_open = false;
                    this.settings_picker("import", None, cx);
                    cx.notify();
                })),
            Button::new("sheet-import-folder")
                .icon(IconName::Folder)
                .label("导入文件夹…")
                .disabled(busy)
                .on_click(cx.listener(|this, _, _, cx| {
                    if this.library_tasks_busy() {
                        return;
                    }
                    this.action_sheet_open = false;
                    this.settings_picker("import-folder", None, cx);
                    cx.notify();
                })),
        ];
        let cancel = Button::new("sheet-cancel")
            .small()
            .ghost()
            .label("取消")
            .on_click(cx.listener(|this, _, _, cx| {
                this.action_sheet_open = false;
                cx.notify();
            }));
        studyapp_ui::modal_frame::ModalFrame::new(
            "新建或导入",
            studyapp_ui::notification_surfaces::action_sheet(
                actions,
                self.effective_appearance()
                    .notifications
                    .layout(NoticeStyle::ActionSheet),
            ),
        )
        .component(NoticeStyle::ActionSheet)
        .footer(div().w_full().child(cancel.w_full()))
        .into_any_element()
    }
}

#[cfg(all(test, feature = "ui-test"))]
mod tests {
    use crate::app::{Page, StudyApp};
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{
        AppContext, Context, Entity, IntoElement, ParentElement, Render, ScrollDelta, Styled,
        TestAppContext, Window, div, point, px,
    };
    use studyapp_core::{Engine, NoticeLayout, NoticeStyle};

    struct Fixture(Entity<StudyApp>);
    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.0.update(cx, |app, cx| {
                div()
                    .size_full()
                    .child(app.library_add_button(cx))
                    .children(app.action_sheet_open.then(|| app.library_action_sheet(cx)))
            })
        }
    }

    #[gpui_kit::test]
    fn sheet_to_folder_form_focuses_the_name_and_enter_submits_once(cx: &mut TestAppContext) {
        use gpui_kit::WindowOptions;
        use std::{cell::RefCell, rc::Rc, time::Duration};

        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::apply(&Default::default(), false, cx);
        });
        let directory = tempfile::tempdir().unwrap().keep();
        let worker = crate::worker::Worker::start(directory.join("studyapp.sqlite3"), None);
        let initial = worker
            .receiver
            .recv_timeout(Duration::from_secs(60))
            .unwrap();
        let snapshot = initial.snapshot.unwrap();
        let (handle, app) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| {
                    let mut app = StudyApp::new(worker, window, cx);
                    app.preferences_ui.saved = initial.preferences;
                    app.accept_snapshot(snapshot, false, window, cx);
                    app.startup_busy = false;
                    app.page = Page::Notes;
                    app
                })
            })
            .unwrap()
        });
        let panel = cx.read(|cx| app.read(cx).folders.clone());
        let commands = Rc::new(RefCell::new(Vec::new()));
        let observed = commands.clone();
        cx.update(|cx| {
            cx.subscribe(
                &panel,
                move |_, event: &crate::folders_ui::FolderEvent, _| {
                    if let crate::folders_ui::FolderEvent::Command(command) = event {
                        observed.borrow_mut().push(command.as_ref().clone());
                    }
                },
            )
            .detach();
        });
        for cancel in [true, false] {
            cx.update_window(handle, |_, window, cx| {
                window.click("library-add-menu", cx);
                assert!(app.read(cx).action_sheet_open);
                window.click("sheet-new-folder", cx);
                assert!(!app.read(cx).action_sheet_open);
                assert!(panel.read(cx).modal_open());
                let name = panel.read(cx).modal_initial_focus(cx).unwrap();
                assert!(
                    name.is_focused(window),
                    "sheet-to-form transition did not focus the name"
                );
                window.input("目录键盘回归", cx);
                if cancel {
                    window.press("escape", cx);
                } else {
                    window.press("enter", cx);
                    window.press("enter", cx);
                }
            })
            .unwrap();
            cx.run_until_parked();
            assert!(!cx.read(|cx| panel.read(cx).modal_open()));
            if cancel {
                assert!(commands.borrow().is_empty());
            }
        }
        assert_eq!(commands.borrow().len(), 1);
        assert!(
            matches!(&commands.borrow()[0], studyapp_core::Command::CreateFolder { name, parent_id: None } if name == "目录键盘回归")
        );
    }

    #[gpui_kit::test]
    fn sheet_operations_ignore_own_modal_but_obey_tasks_and_keep_cancel_fixed(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::apply(&Default::default(), false, cx);
        });
        for (layout, action) in [
            (NoticeLayout::Primary, "sheet-new-note"),
            (NoticeLayout::Alternative, "sheet-new-folder"),
        ] {
            let directory = tempfile::tempdir().unwrap().keep();
            let database = directory.join("studyapp.sqlite3");
            let snapshot = {
                let engine = Engine::open(&database, 100).unwrap();
                std::sync::Arc::new(engine.snapshot())
            };
            let worker = crate::worker::Worker::start(database, None);
            let mut app = None;
            let (_, visual) = cx.add_window_view(|window, cx| {
                let entity = cx.new(|cx| StudyApp::new(worker, window, cx));
                entity.update(cx, |app, cx| {
                    app.page = Page::Notes;
                    app.last_library = snapshot.data_path.clone();
                    app.snapshot = Some(snapshot.clone());
                    app.draft_loading = false;
                    let mut saved = crate::preferences::PreferenceSnapshot::defaults();
                    saved.settings = snapshot.settings.clone();
                    saved
                        .settings
                        .appearance
                        .notifications
                        .layouts
                        .insert(NoticeStyle::ActionSheet, layout);
                    app.preferences_ui.saved = Some(saved);
                    app.folders
                        .update(cx, |folders, cx| folders.accept(snapshot, window, cx));
                });
                cx.observe(&entity, |_, _, cx| cx.notify()).detach();
                app = Some(entity.clone());
                Fixture(entity)
            });
            let app = app.unwrap();
            visual.update(|window, cx| {
                window.render_frame(cx);
                window.click("library-add-menu", cx);
                assert!(app.read(cx).action_sheet_open);
                assert!(app.read(cx).library_controls_busy(cx));
                assert!(!app.read(cx).library_tasks_busy());
                window.render_frame(cx);
                for id in [
                    "sheet-new-note",
                    "sheet-new-folder",
                    "sheet-import-file",
                    "sheet-import-folder",
                ] {
                    assert_eq!(window.find(id).role(), Some(gpui_kit::Role::Button));
                }
                let cancel = window.find("sheet-cancel").bounds();
                let body = window.find("modal-content-scroll").bounds();
                assert!(cancel.top() >= body.bottom());
                window.scroll(
                    "modal-content-scroll",
                    ScrollDelta::Pixels(point(px(0.), px(-200.))),
                    cx,
                );
                assert_eq!(window.find("sheet-cancel").bounds(), cancel);
                window.click("sheet-cancel", cx);
                assert!(!app.read(cx).action_sheet_open);
                assert!(app.read(cx).note_editor.is_none());
                window.click("library-add-menu", cx);
            });
            for busy in 0..4 {
                app.update(visual, |app, cx| {
                    app.storage_busy = busy == 0;
                    app.preferences_ui.busy = busy == 1;
                    app.preferences_ui.replacing = busy == 2;
                    app.note_saving = busy == 3;
                    cx.notify();
                });
                visual.update(|window, cx| {
                    window.render_frame(cx);
                    assert!(app.read(cx).library_tasks_busy());
                    // GPUI does not expose an aria-disabled flag for these buttons.
                    // Verify real pointer inertia instead of treating missing
                    // accessibility metadata as evidence of enabled/disabled state.
                    for id in [
                        "sheet-new-note",
                        "sheet-new-folder",
                        "sheet-import-file",
                        "sheet-import-folder",
                    ] {
                        window.click(id, cx);
                        assert!(app.read(cx).action_sheet_open, "busy action ran: {id}");
                        assert!(app.read(cx).note_editor.is_none());
                        assert!(!app.read(cx).folders.read(cx).modal_open());
                    }
                });
            }
            visual.update(|window, cx| {
                app.update(cx, |app, cx| {
                    app.storage_busy = false;
                    app.preferences_ui.busy = false;
                    app.preferences_ui.replacing = false;
                    app.note_saving = false;
                    app.draft_loading = false;
                    cx.notify();
                });
                window.render_frame(cx);
                window.scroll(
                    "modal-content-scroll",
                    ScrollDelta::Pixels(point(px(0.), px(100_000.))),
                    cx,
                );
                window.click(action, cx);
                assert!(!app.read(cx).action_sheet_open);
                if action == "sheet-new-note" {
                    assert!(app.read(cx).note_editor.is_some());
                } else {
                    assert!(app.read(cx).folders.read(cx).modal_open());
                }
            });
        }
    }
}
