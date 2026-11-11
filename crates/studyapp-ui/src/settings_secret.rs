//! Secret editing never loads the saved secret. Only this session's new input
//! can be revealed. Clear is a confirmed draft change, not immediate IO.
use crate::{components::*, controls::Button};
use gpui_kit::{
    component::{Disableable, Sizable, button::ButtonVariants, input::InputState},
    *,
};
pub struct SecretChanged;
pub struct SecretSetting {
    source: Entity<InputState>,
    saved: bool,
    disabled: bool,
    editing: bool,
    visible: bool,
    confirm_clear: bool,
    clear: bool,
}
impl EventEmitter<SecretChanged> for SecretSetting {}
impl SecretSetting {
    pub fn new(source: Entity<InputState>, saved: bool, cx: &mut Context<Self>) -> Self {
        cx.observe(&source, |_, _, cx| cx.notify()).detach();
        Self {
            source,
            saved,
            disabled: false,
            editing: false,
            visible: false,
            confirm_clear: false,
            clear: false,
        }
    }
    pub fn configure(&mut self, saved: bool, disabled: bool, cx: &mut Context<Self>) {
        if (self.saved, self.disabled) != (saved, disabled) {
            self.saved = saved;
            self.disabled = disabled;
            cx.notify();
        }
    }
    pub fn pending_clear(&self) -> bool {
        self.clear
    }
    pub fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = false;
        self.visible = false;
        self.confirm_clear = false;
        self.clear = false;
        self.source.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.set_masked(true, window, cx);
        });
        cx.notify();
    }
}
impl Render for SecretSetting {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let has_new = !self.source.read(cx).value().is_empty();
        column().gap_2().child(row().flex_wrap().gap_2()
            .child(muted(if has_new { "新凭据待保存" } else if self.clear { "清除凭据待保存" } else if self.saved { "已保存凭据（不会回填）" } else { "尚未保存凭据" }))
            .child(Button::new("secret-replace").small().label(if self.saved { "更换…" } else { "填写…" }).disabled(self.disabled)
                .on_click(cx.listener(|this, _, window, cx| { this.editing = true; this.clear = false; this.source.read(cx).focus_handle(cx).focus(window, cx); cx.notify(); })))
            .child(Button::new("secret-clear").small().ghost().label("清除…").disabled(self.disabled || !self.saved && !has_new)
                .on_click(cx.listener(|this, _, _, cx| { this.confirm_clear = true; cx.notify(); }))))
            .children((self.editing || has_new).then(|| row().flex_wrap().gap_2()
                .child(crate::settings_components::input(&self.source, self.disabled).flex_1().min_w_0())
                .child(Button::new("secret-show-new").small().ghost().label(if self.visible { "隐藏本次输入" } else { "显示本次输入" }).disabled(self.disabled || !has_new)
                    .on_click(cx.listener(|this, _, window, cx| { this.visible = !this.visible; this.source.update(cx, |input, cx| input.set_masked(!this.visible, window, cx)); cx.notify(); })))))
            .children(self.confirm_clear.then(|| column().gap_2().p_3().border_1().border_color(rgb(crate::theme::palette().danger))
                .child(muted("确认清除已保存凭据？这里只修改草稿；保存全部设置后生效。放弃设置会保留原凭据。"))
                .child(row().flex_wrap().gap_2()
                    .child(Button::new("secret-keep").small().ghost().label("保留").on_click(cx.listener(|this, _, _, cx| { this.confirm_clear = false; cx.notify(); })))
                    .child(Button::new("secret-confirm-clear").small().label("确认清除").disabled(self.disabled)
                        .on_click(cx.listener(|this, _, window, cx| { this.clear = true; this.confirm_clear = false; this.editing = false; this.visible = false;
                            this.source.update(cx, |input, cx| { input.set_value("", window, cx); input.set_masked(true, window, cx); }); cx.emit(SecretChanged); cx.notify(); }))))))
    }
}
