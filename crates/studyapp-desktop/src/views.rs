use crate::controls::Button;
use crate::controls::Checkbox;
use crate::{
    app::{EditorMode, Page, StudyApp},
    components::*,
};
use chrono::{Local, TimeZone, Utc};
use gpui_kit::component::{
    Disableable, IconName, Selectable, Sizable,
    button::ButtonVariants,
    input::{Input, Textarea},
    scroll::ScrollableElement,
};
use gpui_kit::{prelude::FluentBuilder, *};
use studyapp_core::{Command, Review, SessionStatus, Snapshot};
use studyapp_desktop::presentation::{duration, stamp};

fn review_text(title: String, metadata: String) -> Div {
    column()
        .flex_1()
        .min_w_0()
        .gap_1()
        .child(
            heading(title)
                .w_full()
                .min_w_0()
                .whitespace_normal()
                .id("review-summary")
                .test_support(),
        )
        .child(muted(metadata).w_full().min_w_0().whitespace_normal())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TodayAction {
    Answer,
    Reviews,
    Record,
}

fn today_action(pending: bool, due: usize) -> TodayAction {
    if pending {
        TodayAction::Answer
    } else if due > 0 {
        TodayAction::Reviews
    } else {
        TodayAction::Record
    }
}

fn today_date() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}
fn day_of(time: i64) -> String {
    Local
        .timestamp_opt(time, 0)
        .single()
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}
fn optional_stamp(time: Option<i64>, empty: &str) -> String {
    time.map(stamp).unwrap_or_else(|| empty.to_owned())
}
fn review_tip(review: &Review) -> String {
    format!(
        "最近学习：{}\n上次复习：{}\n下次复习：{}",
        stamp(review.learned_at),
        optional_stamp(review.last_review_at, "尚未复习"),
        optional_stamp(review.next_review_at, "本轮已完成")
    )
}

impl StudyApp {
    pub fn today(&self, snapshot: &Snapshot, window: &mut Window, cx: &mut Context<Self>) -> Div {
        use crate::design::{MONO_FONT, TypeRole, space};
        use chrono::{Datelike, Timelike};
        let local = Local::now();
        let now = local.timestamp();
        let palette = crate::theme::palette();
        let pending = &self.read_models.pending_sessions;
        let due = self.read_models.due_count(now);
        let action = today_action(!pending.is_empty(), due);
        let pending_id = pending
            .first()
            .map(|&position| snapshot.sessions[position].id.clone());
        let label = match action {
            TodayAction::Answer => "填写学习内容".into(),
            TodayAction::Reviews => format!("开始复习 {due}"),
            TodayAction::Record => "记录学习".into(),
        };
        let primary = Button::new("today-primary")
            .when(self.editor.is_none(), |button| button.primary())
            .when(self.editor.is_some(), |button| button.ghost())
            .label(label)
            .on_click(cx.listener(move |this, _, window, cx| match action {
                TodayAction::Answer => {
                    if let Some(id) = &pending_id {
                        this.open_editor(EditorMode::Pending(id.clone()), window, cx);
                    }
                }
                TodayAction::Reviews => this.navigate(Page::Reviews, window, cx),
                TodayAction::Record => this.open_editor(EditorMode::Manual, window, cx),
            }));
        let greeting = match local.hour() {
            5..=11 => "早上好，今天想学点什么？",
            12..=17 => "下午好，继续积累一点。",
            _ => "晚上好，给今天留一点收获。",
        };
        let mut content = column().min_w_0().gap(space(palette.spacing, 3.)).child(
            page_header(
                local.format("%Y年%m月%d日").to_string(),
                greeting,
                Some("先专注，再用自己的话确认收获。".into()),
                primary,
            )
            .id("today-header")
            .test_support(),
        );
        if !pending.is_empty() {
            content = content.child(
                card()
                    .bg(rgb(palette.warning_soft))
                    .border_color(rgb(palette.warning))
                    .child(
                        row()
                            .child(pill_tone("等待回答", Tone::Warning))
                            .child(muted(format!("{} 段学习需要确认", pending.len()))),
                    )
                    .children(pending.iter().map(|&position| {
                        let session = &snapshot.sessions[position];
                        let id = session.id.clone();
                        row()
                            .flex_wrap()
                            .child(
                                column()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1()
                                    .child(format!("{} 分钟活动时间", session.active_seconds / 60))
                                    .child(muted(format!(
                                        "还有 {} 秒可填写 · {} 开始",
                                        (session.deadline - now).max(0),
                                        stamp(session.started_at)
                                    ))),
                            )
                            .child(
                                Button::new(SharedString::from(format!("answer-{id}")))
                                    .ghost()
                                    .label("填写学习内容")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_editor(
                                            EditorMode::Pending(id.clone()),
                                            window,
                                            cx,
                                        )
                                    })),
                            )
                    })),
            );
        }
        if self.editor.is_some() {
            content = content.child(self.editor_view(snapshot, cx));
        }
        if self.read_models.managed_note_count == 0 {
            return content.child(
                card()
                    .gap(space(palette.spacing, 3.))
                    .child(heading("从一份笔记开始"))
                    .child(muted(
                        "用三个小步骤，把正在学习的内容留在这里。已有学习记录仍可从统计页查看。",
                    ))
                    .child(
                        row()
                            .items_start()
                            .child(pill_tone("1", Tone::Neutral))
                            .child(
                                column()
                                    .flex_1()
                                    .min_w_0()
                                    .child(heading("确认资料库位置"))
                                    .child(muted(
                                        "资料库已经打开；需要更换位置时，可选择其他目录。",
                                    )),
                            )
                            .child(
                                Button::new("onboarding-library")
                                    .ghost()
                                    .label("选择目录")
                                    .on_click(
                                        cx.listener(|this, _, _, _| this.picker("library", None)),
                                    ),
                            ),
                    )
                    .child(
                        row()
                            .items_start()
                            .child(pill_tone("2", Tone::Accent))
                            .child(
                                column()
                                    .flex_1()
                                    .min_w_0()
                                    .child(heading("带入第一份笔记"))
                                    .child(muted(
                                        "导入 Markdown 文件后，版本历史会持续记录内容变化。",
                                    )),
                            )
                            .child(
                                Button::new("first-import")
                                    .ghost()
                                    .label("选择 Markdown 笔记")
                                    .on_click(
                                        cx.listener(|this, _, _, _| this.picker("import", None)),
                                    ),
                            ),
                    )
                    .child(
                        row()
                            .items_start()
                            .child(pill_tone("3", Tone::Neutral))
                            .child(
                                column()
                                    .flex_1()
                                    .min_w_0()
                                    .child(heading("开始一段学习"))
                                    .child(muted(if snapshot.timer.paused {
                                        "准备好后继续计时，结束时确认学到了什么。"
                                    } else {
                                        "计时已经开始。完成后，使用右上角“记录学习”留下收获。"
                                    })),
                            )
                            .child(
                                Button::new("onboarding-start")
                                    .ghost()
                                    .label(if snapshot.timer.paused {
                                        "开始计时"
                                    } else {
                                        "计时进行中"
                                    })
                                    .disabled(!snapshot.timer.paused)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.send(Command::SetPaused(false), cx)
                                    })),
                            ),
                    ),
            );
        }
        let remaining = snapshot
            .timer
            .target_seconds
            .saturating_sub(snapshot.timer.active_seconds);
        let ratio = (snapshot.timer.active_seconds as f32
            / snapshot.timer.target_seconds.max(1) as f32)
            .clamp(0., 1.);
        let paused = snapshot.timer.paused;
        let timer_color = if paused {
            palette.warning
        } else {
            palette.accent
        };
        let segment = self.read_models.record_positions(&today_date(), 0).len() + 1;
        let timer = card()
            .id("today-timer")
            .test_support()
            .min_w_0()
            .child(
                row()
                    .flex_wrap()
                    .child(
                        column()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(heading(if paused {
                                "Ⅱ 计时已暂停"
                            } else {
                                "▶ 专注当下，稍后回顾"
                            }))
                            .child(muted(format!(
                                "第 {segment} 段 · {} 开始",
                                Local
                                    .timestamp_opt(snapshot.timer.started_at, 0)
                                    .single()
                                    .map(|time| time.format("%H:%M").to_string())
                                    .unwrap_or_default()
                            ))),
                    )
                    .child(
                        Button::new("pause")
                            .label(if paused { "继续计时" } else { "暂停" })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.send(Command::SetPaused(!paused), cx)
                            })),
                    ),
            )
            .child(
                typography(TypeRole::Display)
                    .font_family(MONO_FONT)
                    .text_color(rgb(timer_color))
                    .font_features(FontFeatures(std::sync::Arc::new(vec![("tnum".into(), 1)])))
                    .child(duration(remaining)),
            )
            .child(
                div()
                    .id("today-timer-progress")
                    .test_support()
                    .w_full()
                    .h(px(8.))
                    .bg(rgb(palette.inset))
                    .rounded_full()
                    .child(
                        div()
                            .w(relative(ratio))
                            .h_full()
                            .bg(rgb(timer_color))
                            .rounded_full(),
                    ),
            )
            .child(
                row()
                    .flex_wrap()
                    .child(muted(format!(
                        "本段 {} 分钟 · 到时将邀请你确认收获",
                        snapshot.timer.target_seconds / 60
                    )))
                    .child(div().flex_1())
                    .child(
                        Button::new("manual-top")
                            .ghost()
                            .icon(IconName::Plus)
                            .label("记录本段")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_editor(EditorMode::Manual, window, cx)
                            })),
                    ),
            );
        let mut recent = card().min_w_0().child(
            row()
                .child(heading("最近的学习记录"))
                .child(div().flex_1())
                .child(
                    Button::new("all-records")
                        .ghost()
                        .label("查看全部")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.navigate(Page::Stats, window, cx)
                        })),
                ),
        );
        let positions = self.read_models.record_positions("", 0);
        if positions.is_empty() {
            recent = recent.child(muted("完成一段学习后，收获会显示在这里。"));
        }
        let today = local.date_naive();
        let monday = today - chrono::Duration::days(today.weekday().num_days_from_monday() as i64);
        let mut groups: [Vec<usize>; 4] = Default::default();
        for &position in positions.iter().take(6) {
            let date = Local
                .timestamp_opt(snapshot.sessions[position].ended_at, 0)
                .single()
                .map(|time| time.date_naive());
            let group = match date {
                Some(date) if date == today => 0,
                Some(date) if Some(date) == today.pred_opt() => 1,
                Some(date) if date >= monday && date <= today => 2,
                _ => 3,
            };
            groups[group].push(position);
        }
        for (index, label) in ["今天", "昨天", "本周", "更早"].into_iter().enumerate() {
            if !groups[index].is_empty() {
                recent = recent.child(muted(label)).children(
                    groups[index]
                        .iter()
                        .map(|&position| self.session_row(&snapshot.sessions[position], cx)),
                );
            }
        }
        let week_seconds: u64 = self
            .read_models
            .recent_days(today.weekday().num_days_from_monday() as usize + 1)
            .iter()
            .map(|day| day.learned_seconds)
            .sum();
        let checklist = card()
            .id("today-checklist")
            .test_support()
            .min_w_0()
            .child(heading("今日清单"))
            .child(muted(format!(
                "今天已确认 {} 分钟",
                self.read_models.today_learned_seconds() / 60
            )))
            .child(
                Button::new("today-due")
                    .ghost()
                    .label(format!("待复习  {due} 项"))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.navigate(Page::Reviews, window, cx)),
                    ),
            )
            .child(
                Button::new("today-pending")
                    .ghost()
                    .label(format!("待回答  {} 段", pending.len()))
                    .disabled(pending.is_empty())
                    .on_click(cx.listener(|this, _, window, cx| {
                        let id = this
                            .read_models
                            .pending_sessions
                            .first()
                            .and_then(|&position| {
                                this.snapshot
                                    .as_ref()
                                    .and_then(|snapshot| snapshot.sessions.get(position))
                            })
                            .map(|session| session.id.clone());
                        if let Some(id) = id {
                            this.open_editor(EditorMode::Pending(id), window, cx);
                        }
                    })),
            )
            .child(
                Button::new("today-notes")
                    .ghost()
                    .label(format!(
                        "尚未记录的笔记  {} 份",
                        self.read_models.unrecorded_note_count
                    ))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.navigate(Page::Notes, window, cx)),
                    ),
            )
            .child(
                Button::new("today-week")
                    .ghost()
                    .label(format!("本周已确认  {} 分钟", week_seconds / 60))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.navigate(Page::Stats, window, cx)),
                    ),
            );
        let wide = window.viewport_size().width >= px(1180.);
        content.child(if wide {
            row()
                .items_start()
                .gap(space(palette.spacing, 3.))
                .min_w_0()
                .child(
                    column()
                        .flex_1()
                        .min_w_0()
                        .gap(space(palette.spacing, 3.))
                        .child(timer)
                        .child(recent),
                )
                .child(
                    column()
                        .w(relative(1. / 3.))
                        .min_w_0()
                        .flex_shrink_0()
                        .child(checklist),
                )
        } else {
            column()
                .min_w_0()
                .gap(space(palette.spacing, 3.))
                .child(timer)
                .child(checklist)
                .child(recent)
        })
    }
    fn editor_view(&self, snapshot: &Snapshot, cx: &mut Context<Self>) -> Div {
        let Some(mode) = &self.editor else {
            return column();
        };
        let title = match mode {
            EditorMode::Pending(_) => "刚才，你学到了什么？",
            EditorMode::Manual => "补记一次学习",
            EditorMode::Edit(_) => "更正学习记录",
        };
        let mut view = card().border_color(crate::theme::color(GREEN)).child(
            row().child(heading(title)).child(div().flex_1()).child(
                Button::new("cancel-editor")
                    .ghost()
                    .small()
                    .label("收起")
                    .disabled(self.saving)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.editor = None;
                        cx.notify();
                    })),
            ),
        );
        if matches!(mode, EditorMode::Manual) {
            view = view.child(
                row()
                    .child(
                        column()
                            .flex_1()
                            .gap_2()
                            .child(muted("开始时间（本地）"))
                            .child(Input::new(&self.start)),
                    )
                    .child(
                        column()
                            .w(px(160.))
                            .gap_2()
                            .child(muted("学习分钟数"))
                            .child(Input::new(&self.minutes)),
                    ),
            );
        }
        view = view
            .child(Textarea::new(&self.summary).h(px(115.)))
            .child(muted(
                "关联学习笔记（可多选；摘要可以修改，只有提交后才算已学习）",
            ))
            .child(
                row().flex_wrap().children(
                    snapshot
                        .notes
                        .iter()
                        .filter(|note| !note.detached || self.chosen_notes.contains(&note.id))
                        .map(|note| {
                            let id = note.id.clone();
                            let selected = self.chosen_notes.contains(&id);
                            Checkbox::new(SharedString::from(format!("associate-{id}")))
                                .label(note.title.clone())
                                .checked(selected)
                                .on_click(cx.listener(move |this, checked, _, cx| {
                                    this.toggle_note(&id, *checked);
                                    cx.notify();
                                }))
                        }),
                ),
            );
        if !self.chosen_notes.is_empty() {
            view = view
                .child(muted(
                    "确认学习快照（每份笔记至少选择一个版本；时间为实际捕获时间，可多选或预览）",
                ))
                .child(
                    scroll_body("learning-versions", &self.learning_versions_scroll)
                        .max_h(px(220.))
                        .overflow_y_scroll()
                        .children(
                            self.read_models
                                .selected_version_positions(&self.chosen_notes)
                                .into_iter()
                                .map(|position| {
                                    let version = &snapshot.versions[position];
                                    let id = version.id.clone();
                                    let preview_id = id.clone();
                                    let note_id = version.note_id.clone();
                                    let title = self
                                        .read_models
                                        .note_position(&version.note_id)
                                        .and_then(|position| snapshot.notes.get(position))
                                        .map_or("笔记", |n| n.title.as_str());
                                    row()
                                        .child(
                                            Checkbox::new(SharedString::from(format!(
                                                "associate-version-{id}"
                                            )))
                                            .checked(self.chosen_versions.contains(&id))
                                            .label(format!(
                                                "{} · {}{} · {}",
                                                title,
                                                stamp(version.captured_at),
                                                if version.offline { " · 离线" } else { "" },
                                                version
                                                    .summary
                                                    .chars()
                                                    .take(70)
                                                    .collect::<String>()
                                            ))
                                            .on_click(cx.listener(move |this, checked, _, cx| {
                                                if *checked {
                                                    this.chosen_versions.insert(id.clone());
                                                } else {
                                                    this.chosen_versions.remove(&id);
                                                }
                                                cx.notify();
                                            })),
                                        )
                                        .child(
                                            Button::new(SharedString::from(format!(
                                                "preview-learning-{preview_id}"
                                            )))
                                            .ghost()
                                            .small()
                                            .label("预览")
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.show_version(
                                                    note_id.clone(),
                                                    preview_id.clone(),
                                                    window,
                                                    cx,
                                                )
                                            })),
                                        )
                                }),
                        ),
                );
        }
        let skip = match mode {
            EditorMode::Pending(id) => Some(Command::Skip {
                session_id: id.clone(),
            }),
            EditorMode::Edit(id) => Some(Command::EditSession {
                session_id: id.clone(),
                summary: self.summary.read(cx).value().to_string(),
                learned: false,
                note_ids: self.chosen_notes.iter().cloned().collect(),
                version_ids: None,
            }),
            EditorMode::Manual => None,
        };
        view.child(
            row()
                .child(
                    Button::new("save-learning")
                        .primary()
                        .label(if self.saving {
                            "正在保存…"
                        } else {
                            "保存学习记录"
                        })
                        .disabled(self.saving)
                        .on_click(cx.listener(|this, _, _, cx| this.save_editor(cx))),
                )
                .children(skip.map(|command| {
                    Button::new("skip-learning")
                        .ghost()
                        .label("这段时间没有学习")
                        .disabled(self.saving)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.send(command.clone(), cx);
                            this.editor = None;
                        }))
                }))
                .child(muted("提交后自动生成复习计划")),
        )
    }

    fn session_row(&self, session: &studyapp_core::Session, cx: &mut Context<Self>) -> Div {
        let (label, tone) = match session.status {
            SessionStatus::Learned => ("已学习", Tone::Success),
            SessionStatus::Missed => ("未学习", Tone::Neutral),
            SessionStatus::Pending => ("待回答", Tone::Warning),
        };
        let mode = if session.status == SessionStatus::Pending {
            EditorMode::Pending(session.id.clone())
        } else {
            EditorMode::Edit(session.id.clone())
        };
        row()
            .group("session-row")
            .py_3()
            .border_b_1()
            .border_color(crate::theme::color(BORDER))
            .child(pill_tone(label, tone))
            .child(
                column()
                    .gap_1()
                    .flex_1()
                    .min_w_0()
                    .child(div().truncate().child(if session.summary.is_empty() {
                        "尚未填写学习内容".into()
                    } else {
                        session.summary.chars().take(160).collect::<String>()
                    }))
                    .child(muted(format!(
                        "{}  ·  {} 分钟  ·  {} 份关联笔记 · {} 个快照",
                        stamp(session.started_at),
                        session.active_seconds / 60,
                        session.note_ids.len(),
                        session.version_ids.len()
                    ))),
            )
            .child(
                Button::new(SharedString::from(format!("edit-{}", session.id)))
                    .ghost()
                    .small()
                    .opacity(0.)
                    .group_hover("session-row", |style| style.opacity(1.))
                    .focus_visible(|style| style.opacity(1.))
                    .label("编辑")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_editor(mode.clone(), window, cx)
                    })),
            )
    }

    pub fn reviews(&self, snapshot: &Snapshot, cx: &mut Context<Self>) -> Div {
        let now = Utc::now().timestamp();
        let today = today_date();
        let mut content = column()
            .min_w_0()
            .child(
                page_header(
                    today.clone(),
                    "把记忆再巩固一次",
                    Some(
                        format!(
                            "现在有 {} 项待复习；完成后会安排下一次回顾。",
                            self.read_models.due_count(now)
                        )
                        .into(),
                    ),
                    div(),
                )
                .id("reviews-header")
                .test_support(),
            )
            .child(
                row().flex_wrap().children(
                    ["待复习与逾期", "今日已完成", "全部计划"]
                        .into_iter()
                        .enumerate()
                        .map(|(i, label)| {
                            Button::new(("review-filter", i))
                                .label(label)
                                .selected(self.review_filter == i)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.review_filter = i;
                                    cx.notify();
                                }))
                        }),
                ),
            );
        let reviews = self.read_models.review_positions(self.review_filter, now);
        if reviews.is_empty() {
            content = content.child(empty(
                "当前没有复习任务",
                "学习确认后会自动安排复习。到期后勾选完成，下一次时间随之更新。",
            ));
        }
        for &position in reviews {
            let review = &snapshot.reviews[position];
            let id = review.id.clone();
            let stage = review.stage;
            let tip = review_tip(review);
            let due = review.next_review_at.is_some_and(|t| t <= now);
            let done_today = self.read_models.review_completed_today(&id);
            let complete = review.next_review_at.is_none();
            let mut item = card()
                .id(SharedString::from(format!("review-card-{id}")))
                .test_support()
                .tooltip(move |window, cx| pointer_tooltip(tip.clone(), window, cx))
                .w_full()
                .min_w_0()
                .child(
                    row()
                        .w_full()
                        .min_w_0()
                        .items_start()
                        .child(
                            div().flex_shrink_0().child(
                                Checkbox::new(SharedString::from(format!("review-check-{id}")))
                                    .checked(complete || (done_today && !due))
                                    .disabled(!due && !done_today)
                                    .accessibility_label("标记复习完成或撤销")
                                    .on_click(cx.listener(move |this, checked, _, cx| {
                                        this.send(
                                            if *checked {
                                                Command::CompleteReview {
                                                    review_id: id.clone(),
                                                    expected_stage: stage,
                                                }
                                            } else {
                                                Command::UndoReview {
                                                    review_id: id.clone(),
                                                }
                                            },
                                            cx,
                                        );
                                    })),
                            ),
                        )
                        .child(review_text(
                            review.title.clone(),
                            format!(
                                "学习于 {} · 已完成 {}/{} 次复习",
                                stamp(review.learned_at),
                                review.stage,
                                review.intervals.len()
                            ),
                        ))
                        .child(
                            pill_tone(
                                if complete {
                                    "本轮已完成"
                                } else if due {
                                    if review.next_review_at.is_some_and(|t| day_of(t) != today) {
                                        "已逾期"
                                    } else {
                                        "现在复习"
                                    }
                                } else {
                                    "已安排"
                                },
                                if complete {
                                    Tone::Success
                                } else if due {
                                    Tone::Warning
                                } else {
                                    Tone::Neutral
                                },
                            )
                            .flex_shrink_0()
                            .id(SharedString::from(format!("review-status-{}", review.id)))
                            .test_support(),
                        ),
                )
                .child(
                    row()
                        .flex_wrap()
                        .children(review.intervals.iter().enumerate().map(|(i, days)| {
                            pill_tone(
                                format!(
                                    "{} · {} 天",
                                    if i < review.stage { "✓" } else { "○" },
                                    days
                                ),
                                if i < review.stage {
                                    Tone::Success
                                } else {
                                    Tone::Neutral
                                },
                            )
                        })),
                )
                .child(muted(format!(
                    "上次：{}    下次：{}",
                    optional_stamp(review.last_review_at, "尚未复习"),
                    optional_stamp(review.next_review_at, "本轮已完成")
                )));
            let review_id = review.id.clone();
            if review.last_review_at.is_some() {
                item = item.child(
                    Button::new(SharedString::from(format!("undo-{review_id}")))
                        .ghost()
                        .small()
                        .label("撤销最近一次复习")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.send(
                                Command::UndoReview {
                                    review_id: review_id.clone(),
                                },
                                cx,
                            )
                        })),
                );
            }
            content = content.child(item);
        }
        content.child(muted(
            "默认间隔 1、3、7、14、30 天；每次完成后，从实际完成时间计算下一次复习。",
        ))
    }

    pub fn statistics(&self, snapshot: &Snapshot, cx: &mut Context<Self>) -> Div {
        use crate::design::{TypeRole, line_height, space, type_size};
        let palette = crate::theme::palette();
        let header = page_header(
            "每一次确认，都会留下积累",
            "学习统计",
            Some("观察节奏与变化，找到适合自己的学习步调。".into()),
            Button::new("stats-record")
                .ghost()
                .label("记录学习")
                .on_click(cx.listener(|this, _, window, cx| {
                    this.open_editor(EditorMode::Manual, window, cx)
                })),
        )
        .id("statistics-header")
        .test_support();
        if snapshot.sessions.is_empty() {
            return column()
                .min_w_0()
                .child(header)
                .child(empty(
                    "你的第一份学习记录，会从这里开始",
                    "完成一段学习并确认收获，或手动补记已完成的学习，就能看到每日时长和复习积累。",
                ))
                .child(
                    Button::new("stats-first-record")
                        .primary()
                        .label("补记一次学习")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_editor(EditorMode::Manual, window, cx)
                        })),
                );
        }
        let days = self.read_models.recent_days(self.stats_days);
        let total = self.read_models.learned_seconds;
        let learned = self.read_models.learned_count;
        let average =
            days.iter().map(|day| day.learned_seconds).sum::<u64>() / days.len().max(1) as u64;
        let caption_height =
            type_size(palette.font_size, TypeRole::Caption) * line_height(TypeRole::Caption);
        let max = days
            .iter()
            .map(|d| d.learned_seconds)
            .max()
            .unwrap_or(0)
            .max(60);
        let mut content = column()
            .min_w_0()
            .child(header)
            .child(
                row()
                    .flex_wrap()
                    .items_stretch()
                    .child(metric(
                        "累计学习",
                        format!("{:.1} 小时", total as f64 / 3600.),
                        "所有已确认学习记录",
                    ))
                    .child(metric(
                        "学习记录",
                        format!("{learned} 次"),
                        "未回答时间段不计入",
                    ))
                    .child(metric(
                        "累计复习",
                        format!("{} 次", snapshot.review_events.len()),
                        "已完成且未撤销的复习",
                    )),
            )
            .child(
                card()
                    .child(
                        row()
                            .flex_wrap()
                            .child(heading("每日学习时长"))
                            .child(div().flex_1())
                            .children([7, 30, 90].into_iter().map(|days| {
                                Button::new(("stats-days", days))
                                    .small()
                                    .ghost()
                                    .selected(self.stats_days == days)
                                    .label(format!("{days} 天"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.stats_days = days;
                                        cx.notify();
                                    }))
                            })),
                    )
                    .child(muted(format!(
                        "最高 {} 分钟 / 天 · 日均线 {:.1} 分钟 · 悬停查看完整日期与明细",
                        max / 60,
                        average as f64 / 60.
                    )))
                    .child(
                        row()
                            .id("study-bars")
                            .test_support()
                            .relative()
                            .track_scroll(&self.statistics_scroll)
                            .horizontal_scrollbar(&self.statistics_scroll)
                            .overflow_x_scroll()
                            .pb(space(palette.spacing, 1.5))
                            .items_end()
                            .gap(space(palette.spacing, 1.))
                            .h(px(145.) + caption_height + space(palette.spacing, 2.5))
                            .child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .right_0()
                                    .bottom(
                                        caption_height
                                            + space(palette.spacing, 2.5)
                                            + px(average as f32 / max as f32 * 145.),
                                    )
                                    .h(px(1.))
                                    .bg(rgb(palette.chart[2])),
                            )
                            .children(days.iter().enumerate().map(|(i, stat)| {
                                let height =
                                    (stat.learned_seconds as f32 / max as f32 * 145.).max(2.);
                                let tip = format!(
                                    "{}\n学习 {} 分钟 · {} 次记录\n未学习 {} 段 · 复习 {} 次",
                                    stat.date,
                                    stat.learned_seconds / 60,
                                    stat.learned_count,
                                    stat.missed_count,
                                    stat.review_count
                                );
                                column()
                                    .id(("bar", i))
                                    .test_support()
                                    .w(type_size(palette.font_size, TypeRole::Caption) * 3.8)
                                    .flex_shrink_0()
                                    .gap(space(palette.spacing, 1.))
                                    .items_center()
                                    .justify_end()
                                    .h_full()
                                    .tab_index(0)
                                    .aria_label(tip.clone())
                                    .focus_visible(|style| style.bg(rgb(palette.inset)))
                                    .tooltip(move |window, cx| {
                                        pointer_tooltip(tip.clone(), window, cx)
                                    })
                                    .child(div().w(px(18.)).h(px(height)).rounded_t_md().bg(rgb(
                                        if stat.learned_seconds > 0 {
                                            palette.chart[3]
                                        } else {
                                            palette.border
                                        },
                                    )))
                                    .child(
                                        typography(TypeRole::Caption)
                                            .text_color(crate::theme::color(MUTED))
                                            .child(
                                                stat.date
                                                    .get(5..)
                                                    .unwrap_or(&stat.date)
                                                    .replace('-', "/"),
                                            ),
                                    )
                            })),
                    ),
            );
        let heat = self.read_models.calendar();
        let maximum = heat
            .iter()
            .map(|d| d.review_count)
            .max()
            .unwrap_or(1)
            .max(1);
        content = content.child(
            row()
                .flex_wrap()
                .items_stretch()
                .child(
                    card()
                        .flex_1()
                        .min_w(px(300.))
                        .child(heading("最近 13 周的积累"))
                        .child(muted("颜色越深，当天确认的学习时间越长"))
                        .child(row().gap_1().children(heat.chunks(7).enumerate().map(
                            |(week, chunk)| {
                                column().gap_1().children(chunk.iter().enumerate().map(
                                    move |(day, stat)| {
                                        let color = match stat.learned_seconds {
                                            0 => palette.border,
                                            1..=1799 => palette.chart[0],
                                            1800..=3599 => palette.chart[1],
                                            3600..=7199 => palette.chart[2],
                                            _ => palette.chart[3],
                                        };
                                        let tip = format!(
                                            "{} · {} 分钟",
                                            stat.date,
                                            stat.learned_seconds / 60
                                        );
                                        div()
                                            .id(("heat", week * 7 + day))
                                            .size(px(18.))
                                            .rounded_sm()
                                            .bg(rgb(color))
                                            .tab_index(0)
                                            .aria_label(tip.clone())
                                            .focus_visible(|style| {
                                                style.border_1().border_color(rgb(palette.focus))
                                            })
                                            .tooltip(move |w, cx| {
                                                pointer_tooltip(tip.clone(), w, cx)
                                            })
                                    },
                                ))
                            },
                        ))),
                )
                .child(
                    card()
                        .flex_1()
                        .min_w(px(300.))
                        .child(heading("最近两周的复习"))
                        .child(muted("每天完成的复习次数"))
                        .child(row().items_end().gap_2().h(px(140.)).children(
                            heat.iter().skip(77).enumerate().map(|(i, stat)| {
                                let tip = format!("{} · {} 次复习", stat.date, stat.review_count);
                                column()
                                    .id(("review-bar", i))
                                    .flex_1()
                                    .h_full()
                                    .justify_end()
                                    .items_center()
                                    .gap_2()
                                    .tab_index(0)
                                    .aria_label(tip.clone())
                                    .tooltip(move |w, cx| pointer_tooltip(tip.clone(), w, cx))
                                    .child(
                                        typography(TypeRole::Caption)
                                            .child(stat.review_count.to_string()),
                                    )
                                    .child(
                                        div()
                                            .w(px(13.))
                                            .h(px((stat.review_count as f32 / maximum as f32
                                                * 85.)
                                                .max(2.)))
                                            .rounded_t_md()
                                            .bg(rgb(palette.chart[2])),
                                    )
                                    .child(
                                        typography(TypeRole::Caption)
                                            .text_color(crate::theme::color(MUTED))
                                            .child(
                                                stat.date.get(8..).unwrap_or(&stat.date).to_owned(),
                                            ),
                                    )
                            }),
                        )),
                ),
        );
        let record_date = self.record_date.read(cx).value();
        let record_date = record_date.trim();
        let sessions = self
            .read_models
            .record_positions(record_date, self.record_filter);
        content.child(
            card()
                .child(
                    row()
                        .flex_wrap()
                        .child(heading(format!("全部学习记录 · {} 条", sessions.len())))
                        .child(
                            div()
                                .id("record-date-filter")
                                .w(px(210.))
                                .child(Input::new(&self.record_date).cleanable(true)),
                        )
                        .child(div().flex_1())
                        .children(
                            ["全部", "已学习", "未学习", "待回答"]
                                .into_iter()
                                .enumerate()
                                .map(|(i, label)| {
                                    Button::new(("record-filter", i))
                                        .ghost()
                                        .small()
                                        .selected(self.record_filter == i)
                                        .label(label)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.record_filter = i;
                                            this.record_limit = 50;
                                            cx.notify();
                                        }))
                                }),
                        ),
                )
                .when(sessions.is_empty(), |c| {
                    c.child(muted("当前筛选下还没有记录。"))
                })
                .children(
                    sessions
                        .iter()
                        .take(self.record_limit)
                        .map(|&position| self.session_row(&snapshot.sessions[position], cx)),
                )
                .when(sessions.len() > self.record_limit, |c| {
                    c.child(Button::new("more-records").label("再显示 50 条").on_click(
                        cx.listener(|this, _, _, cx| {
                            this.record_limit += 50;
                            cx.notify();
                        }),
                    ))
                }),
        )
    }

    pub fn settings(
        &self,
        snapshot: &Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        use crate::settings_ui::{SettingsSection, StoragePage};
        let compact = window.viewport_size().width < px(1120.);
        if self.settings_search_active(cx) {
            return column()
                .h_full()
                .min_h_0()
                .min_w_0()
                .gap_3()
                .child(
                    div()
                        .id("settings-save-header")
                        .test_support()
                        .flex_shrink_0()
                        .child(self.settings_header(cx)),
                )
                .child(self.settings_search_bar(cx))
                .child(
                    scroll_body("settings-search-results", &self.settings_scroll)
                        .flex_1()
                        .pr_4()
                        .pb_4()
                        .child(self.settings_search_results(cx)),
                );
        }
        let section = self.preferences_ui.section;
        let content = column().min_w_0().w_full().max_w(px(960.)).flex_shrink_0().gap_3().child(self.preference_controls(cx))
            .children((section == SettingsSection::Storage && self.preferences_ui.storage_page == StoragePage::Library).then(|| self.library_storage_controls(&snapshot.data_path, cx)))
            .children((section == SettingsSection::Storage && matches!(self.preferences_ui.storage_page, StoragePage::Overview | StoragePage::Copies)).then(|| self.storage_view(cx)))
            .children((section == SettingsSection::Sync).then(|| self.sync_panel.clone()))
            .children((section == SettingsSection::AboutAccount).then(|| self.update_panel.clone()))
            .children((section == SettingsSection::AboutAccount).then(|| card().child(heading("账户与微信登录"))
                .child(row().child(pill("本地访客", LIGHT)).child(Button::new("wechat-disabled").label("微信登录 · 尚未连接服务").disabled(true)))
                .child(muted("无需登录即可使用本地功能及自己的文件同步服务。微信登录仍保留后续接入。"))))
            .children((section == SettingsSection::AboutAccount).then(|| card().child(heading("文件历史与隐私"))
                .child(muted("新建笔记保存在资料库 notes 目录，保存时自动生成版本；外部导入文件保持只读。运行时约每 3 秒检查文件改动，快速连续保存会合并为稳定版本。"))
                .child(muted("离线期间只能比较上次快照与当前内容，无法还原中间的每一次保存。文件变化用于建议摘要，必须由你确认才会记为学习。"))
                .child(muted("笔记预览支持图片，原始 HTML 不会执行；学习时长来自你的确认，不代表对实际专注的自动测量。"))));
        let transitioning = !cx.reduce_motion()
            && self
                .preferences_ui
                .section_changed_at
                .is_some_and(|changed| changed.elapsed() < std::time::Duration::from_millis(140));
        let content = if transitioning {
            content
                .with_animation(
                    (
                        "settings-section-transition",
                        self.preferences_ui.section_revision as usize,
                    ),
                    Animation::new(std::time::Duration::from_millis(120)),
                    |content, progress| content.opacity(0.94 + 0.06 * progress),
                )
                .into_any_element()
        } else {
            content.into_any_element()
        };
        let workspace = if compact {
            column()
        } else {
            row().items_stretch()
        };
        column()
            .h_full()
            .min_h_0()
            .min_w_0()
            .gap_2()
            .child(
                div()
                    .id("settings-save-header")
                    .test_support()
                    .flex_shrink_0()
                    .child(self.settings_header(cx)),
            )
            .child(self.settings_search_bar(cx))
            .child(
                workspace
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .gap_3()
                    .child(
                        self.settings_navigation(compact, cx)
                            .id("settings-category-navigation")
                            .test_support()
                            .flex_shrink_0(),
                    )
                    .child(
                        column()
                            .flex_1()
                            .min_h_0()
                            .min_w_0()
                            .gap_2()
                            .child(self.settings_task_navigation(cx))
                            .child(if section == SettingsSection::Shortcuts {
                                self.shortcut_controls(cx).into_any_element()
                            } else if section == SettingsSection::Appearance {
                                self.appearance_surface(cx).into_any_element()
                            } else {
                                scroll_body("settings-section-scroll", &self.settings_scroll)
                                    .test_support()
                                    .flex_1()
                                    .min_h_0()
                                    .min_w_0()
                                    .pr_4()
                                    .pb_4()
                                    .child(content)
                                    .into_any_element()
                            }),
                    ),
            )
    }
}

#[cfg(test)]
mod today_presentation_tests {
    use super::{TodayAction, today_action};

    #[test]
    fn pending_answer_takes_priority_over_due_review() {
        assert_eq!(today_action(true, 5), TodayAction::Answer);
        assert_eq!(today_action(true, 0), TodayAction::Answer);
    }

    #[test]
    fn due_review_precedes_manual_record_without_pending_answer() {
        assert_eq!(today_action(false, 3), TodayAction::Reviews);
        assert_eq!(today_action(false, 0), TodayAction::Record);
    }
}

#[cfg(all(test, feature = "ui-test"))]
mod review_layout_tests {
    use super::review_text;
    use crate::components::{card, row};
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{
        Context, InteractiveElement, IntoElement, ParentElement, Render, Styled, TestAppContext,
        TestSupportExt, Window, div, px,
    };

    struct ReviewFixture {
        width: f32,
        title: String,
    }
    impl Render for ReviewFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            card()
                .id("fixture-card")
                .test_support()
                .w(px(self.width))
                .child(
                    row()
                        .w_full()
                        .min_w_0()
                        .items_start()
                        .child(
                            div()
                                .id("fixture-check")
                                .test_support()
                                .size(px(20.))
                                .flex_shrink_0(),
                        )
                        .child(review_text(self.title.clone(), "Learning metadata".into()))
                        .child(
                            div()
                                .id("fixture-status")
                                .test_support()
                                .w(px(80.))
                                .h(px(24.))
                                .flex_shrink_0(),
                        ),
                )
        }
    }
    #[gpui_kit::test]
    fn review_summary_contains_unbroken_and_mixed_text(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::apply(&Default::default(), false, cx);
        });
        let title = format!(
            "{}中文🧠{}",
            "UnbrokenSummary".repeat(30),
            "混合长摘要".repeat(20)
        );
        let (view, test_cx) = cx.add_window_view(|_, _| ReviewFixture {
            width: 360.,
            title: title.clone(),
        });
        let mut narrow_height = px(0.);
        for width in [360., 770.] {
            test_cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.width = width;
                    cx.notify();
                });
                window.render_frame(cx);
                let summary = window.find("review-summary").bounds();
                let check = window.find("fixture-check").bounds();
                let status = window.find("fixture-status").bounds();
                let card = window.find("fixture-card").bounds();
                assert!(
                    summary.right() <= status.left(),
                    "summary overlaps status: {summary:?}, {status:?}"
                );
                assert!(summary.left() >= check.right());
                assert!(status.right() <= card.right());
                assert!(
                    summary.size.height > px(50.),
                    "full summary must wrap and grow"
                );
                assert_eq!(view.read(cx).title, title);
                if width == 360. {
                    narrow_height = summary.size.height;
                } else {
                    assert!(summary.size.height < narrow_height);
                }
            });
        }
    }
}
