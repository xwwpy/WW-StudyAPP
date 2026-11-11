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
                    assert_ne!(
                        window.find(id).disabled(),
                        Some(true),
                        "sheet disabled its own {id}"
                    );
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
                    assert_eq!(window.find("sheet-new-note").disabled(), Some(true));
                    window.click("sheet-new-note", cx);
                    assert!(app.read(cx).action_sheet_open);
                    assert!(app.read(cx).note_editor.is_none());
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
