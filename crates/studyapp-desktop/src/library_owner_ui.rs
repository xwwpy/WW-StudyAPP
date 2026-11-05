//! Owner recovery uses the same normal-close/unsaved/draft barriers as the titlebar.
use crate::{
    app::StudyApp,
    components::*,
    controls::Button,
    library_owner::{Control, Phase, Progress},
    worker::Work,
};
use gpui_kit::{
    component::{Disableable, button::ButtonVariants},
    *,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
#[derive(Default)]
pub(crate) struct Recovery {
    pub review: bool,
    pub waiting: bool,
    epoch: u64,
}
impl Recovery {
    pub fn reset(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.review = false;
        self.waiting = false;
    }
}
impl StudyApp {
    pub(crate) fn owner_controls(&self, cx: &Context<Self>) -> AnyElement {
        let Some(offer) = self.startup_failure.as_ref().and_then(|f| f.owner.as_ref()) else {
            return muted(
                "当前无法可靠核实占用者。请手动关闭对应应用后重试；不会关闭未知进程或删除锁文件。",
            )
            .into_any_element();
        };
        if self.owner_recovery.waiting {
            return column()
                .gap_2()
                .child(muted(
                    "已请求正常关闭。若有未保存内容，请回到占用窗口确认；不会抢占该窗口焦点。",
                ))
                .child(
                    Button::new("owner-stop-waiting")
                        .ghost()
                        .label("停止等待")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.owner_recovery.reset();
                            this.startup_busy = false;
                            cx.notify();
                        })),
                )
                .child(muted("停止等待不会撤回已发出的正常关闭请求。"))
                .into_any_element();
        }
        if self.owner_recovery.review {
            return card().gap_2().child(heading("确认正常关闭占用应用"))
                .child(div().child(format!("已核实为同一账户、程序身份一致的 WW-StudyApp（进程 {}），且关联当前资料库锁。",offer.pid)))
                .child(muted("仅请求正常关闭，不会强制终止。对方仍可处理未保存内容或取消关闭；锁实际释放后才重新打开资料库。"))
                .child(row().child(Button::new("owner-cancel-review").ghost().label("取消").on_click(cx.listener(|this,_,_,cx|{this.owner_recovery.reset();cx.notify();})))
                    .child(Button::new("owner-confirm-close").primary().label("请求正常关闭并重试").disabled(self.startup_busy)
                        .on_click(cx.listener(|this,_,_,cx|this.begin_owner_retry(cx)))))
                .into_any_element();
        }
        Button::new("startup-close-owner")
            .label("关闭占用应用并重试")
            .disabled(self.startup_busy)
            .on_click(cx.listener(|this, _, _, cx| {
                this.owner_recovery.review = true;
                cx.notify();
            }))
            .into_any_element()
    }
    fn begin_owner_retry(&mut self, cx: &mut Context<Self>) {
        if self.startup_busy {
            return;
        }
        let Some(offer) = self.startup_failure.as_ref().and_then(|f| f.owner.clone()) else {
            return;
        };
        self.owner_recovery.reset();
        self.owner_recovery.waiting = true;
        self.startup_busy = true;
        let epoch = self.owner_recovery.epoch;
        let ticket = uuid::Uuid::new_v4().simple().to_string();
        cx.spawn(async move |this, cx| {
            let started = Instant::now();
            let mut first = true;
            loop {
                let peer = offer.clone();
                let id = ticket.clone();
                let outcome = cx.background_executor().spawn(async move {
                    if first { peer.request_close(&id) } else { peer.progress(&id) }
                }).await;
                let keep = this.update(cx, |this, cx| {
                    if this.owner_recovery.epoch != epoch || !this.owner_recovery.waiting {
                        return false;
                    }
                    match outcome {
                        Ok(Progress::Released) => {
                            this.owner_recovery.reset();
                            this.startup_busy = false;
                            this.startup_work(Work::RetryOpenLibrary, cx);
                            false
                        }
                        Ok(Progress::Phase(Phase::Requested | Phase::Waiting | Phase::Closing)
                            | Progress::WaitingForRelease)
                            if started.elapsed() < Duration::from_secs(120) => true,
                        result => {
                            this.owner_recovery.reset();
                            this.startup_busy = false;
                            let message = match result {
                                Ok(Progress::Phase(Phase::Cancelled)) => "占用应用已取消关闭，资料库仍由其使用。可在对方窗口保存后重试。",
                                Ok(Progress::Phase(Phase::Busy)) => "占用应用正在切换资料库或处理其他操作，请稍后重试。",
                                Ok(_) => "等待关闭已超时。未强制结束任何进程，请处理占用窗口后重试。",
                                Err(_) => "占用者身份、连接或锁状态无法继续核实。未强制关闭任何进程，请手动核对后重试。",
                            };
                            this.push_notice(Err(message.into()), cx);
                            cx.notify();
                            false
                        }
                    }
                }).unwrap_or(false);
                if !keep { break; }
                first = false;
                cx.background_executor().timer(Duration::from_millis(400)).await;
            }
        }).detach();
        cx.notify();
    }
    pub(crate) fn accept_owner_close(
        &mut self,
        control: Arc<Control>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.snapshot.is_none()
            || std::path::Path::new(&self.last_library) != control.database
            || self.storage_busy
            || self.preferences_ui.busy
            || self.preferences_ui.replacing
            || self.owner_close.is_some()
        {
            control.mark(Phase::Busy);
            return;
        }
        control.mark(Phase::Waiting);
        self.owner_close = Some(control);
        // Explicit exit, not the timer-widget titlebar's hide/minimize action.
        self.timer_exit_requested = true;
        self.handle_close(window, cx);
        cx.notify();
    }
    pub(crate) fn update_owner_close(&mut self) {
        if let Some(control) = &self.owner_close {
            if self.close_allowed {
                control.mark(Phase::Closing);
            } else if !self.close_requested && self.preferences_ui.leave.is_none() {
                control.mark(Phase::Cancelled);
                self.owner_close = None;
            }
        }
    }
}
