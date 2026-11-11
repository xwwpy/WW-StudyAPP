//! User-invoked real operations; destructive operations keep their own confirms.
use crate::{app::StudyApp, controls::Button};
use gpui_kit::{
    component::{Disableable, IconName, Sizable, button::ButtonVariants},
    *,
};
use studyapp_core::NoticeStyle;
impl StudyApp {
    pub(crate) fn library_action_sheet(&self, cx: &Context<Self>) -> AnyElement {
        let busy = self.library_controls_busy(cx);
        let actions = vec![
            Button::new("sheet-new-note")
                .icon(IconName::FileText)
                .label("新建 Markdown")
                .disabled(busy)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.action_sheet_open = false;
                    this.new_note(window, cx);
                })),
            Button::new("sheet-new-folder")
                .icon(IconName::Folder)
                .label("新建文件夹")
                .disabled(busy || !self.folders.read(cx).can_create_folder())
                .on_click(cx.listener(|this, _, window, cx| {
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
                    this.action_sheet_open = false;
                    this.settings_picker("import", None, cx);
                    cx.notify();
                })),
            Button::new("sheet-import-folder")
                .icon(IconName::Folder)
                .label("导入文件夹…")
                .disabled(busy)
                .on_click(cx.listener(|this, _, _, cx| {
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
                cancel,
                self.effective_appearance()
                    .notifications
                    .layout(NoticeStyle::ActionSheet),
            ),
        )
        .component(NoticeStyle::ActionSheet)
        .into_any_element()
    }
}
