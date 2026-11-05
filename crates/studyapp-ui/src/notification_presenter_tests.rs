use super::{NotificationActionRequested, NotificationCenter};
use gpui_kit::{
    AppContext, Bounds, Context, InteractiveElement, IntoElement, ParentElement, Render, Styled,
    TestAppContext, TestSupportExt, Window, div, point, px, size, test::TestWindowExt,
};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc, time::Duration};
use studyapp_core::{
    NoticeAction, NoticeCategory, NoticeEnvironment, NoticeEvent, NoticeRule, NoticeSeverity,
    NoticeStyle, NotificationAppearance, NotificationSettings,
};

fn environment(foreground: bool) -> NoticeEnvironment {
    NoticeEnvironment {
        foreground,
        desktop_available: false,
        anchor_available: false,
        minute: 720,
    }
}
fn event() -> NoticeEvent {
    let mut e = NoticeEvent::new(
        NoticeCategory::IntegrityResult,
        NoticeSeverity::Error,
        "检查未完成，请稍后重试。",
    );
    e.action = Some(NoticeAction::OpenIntegrity);
    e
}

#[gpui_kit::test]
fn foreground_styles_keep_real_card_occlusions_and_explicit_actions(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::apply(&Default::default(), false, cx);
    });
    let (center, visual) = cx.add_window_view(|_, _| NotificationCenter::new());
    visual.simulate_resize(size(px(800.), px(700.)));
    let output = Rc::new(RefCell::new(Vec::new()));
    let capture = output.clone();
    let _subscription = visual.update(|_, cx| {
        cx.subscribe(&center, move |_, e: &NotificationActionRequested, _| {
            capture.borrow_mut().push((e.event_id, e.action))
        })
    });
    for style in [
        NoticeStyle::Bubble,
        NoticeStyle::CornerCard,
        NoticeStyle::Banner,
        NoticeStyle::Anchored,
    ] {
        let mut policy = NotificationSettings::default();
        policy.rules.insert(
            NoticeCategory::IntegrityResult,
            NoticeRule {
                style: Some(style),
                ..Default::default()
            },
        );
        let appearance = NotificationAppearance {
            reduce_motion: true,
            ..Default::default()
        };
        let e = event();
        let event_id = e.id;
        e.validate().unwrap();
        center.update(visual, |center, cx| {
            center.clear(cx);
            center.configure(&policy, &appearance, environment(true), false, cx);
            center.set_anchors(
                BTreeMap::from([(
                    "integrity".into(),
                    Bounds::new(point(px(100.), px(120.)), size(px(150.), px(32.))),
                )]),
                cx,
            );
            let mut e = e;
            e.anchor = Some("integrity".into());
            assert_eq!(center.enqueue(e, cx), Some(event_id));
            assert!(center.timer.is_none());
        });
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let id = center.read_with(visual, |center, _| {
            center.queue.visible().next().unwrap().id
        });
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            let bounds = window.find(format!("notice-{id}")).bounds();
            assert!(bounds.size.width > px(100.));
            assert!(bounds.left() >= px(0.) && bounds.right() <= px(800.));
            assert!(bounds.top() >= px(0.) && bounds.bottom() <= px(700.));
            assert_eq!(center.read(cx).occlusions(), &[bounds]);
            window.click(format!("notice-action-{id}"), cx);
        });
        visual.run_until_parked();
        assert!(center.read_with(visual, |center, _| center.queue.is_empty()));
        assert_eq!(
            output.borrow().last(),
            Some(&(event_id, NoticeAction::OpenIntegrity))
        );
    }
}

#[gpui_kit::test]
fn modal_waits_behind_required_confirmation_without_expiry_or_idle_polling(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_kit::init);
    struct ModalFixture {
        center: gpui_kit::Entity<NotificationCenter>,
    }
    impl Render for ModalFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().children(
                self.center
                    .update(cx, |center, cx| center.modal_content(cx)),
            )
        }
    }
    let mut center = None;
    let (_, visual) = cx.add_window_view(|_, cx| {
        let value = cx.new(|_| NotificationCenter::new());
        center = Some(value.clone());
        ModalFixture { center: value }
    });
    let center = center.unwrap();
    let appearance = NotificationAppearance {
        reduce_motion: true,
        ..Default::default()
    };
    center.update(visual, |center, cx| {
        center.freeze_time();
        center.configure(
            &NotificationSettings::default(),
            &appearance,
            environment(true),
            true,
            cx,
        );
        let mut e = event();
        e.severity = NoticeSeverity::Critical;
        center.enqueue(e, cx);
        center.advance_time(Duration::from_secs(3600), cx);
        assert!(!center.modal_visible());
        assert!(center.timer.is_none());
        center.configure(
            &NotificationSettings::default(),
            &appearance,
            environment(true),
            false,
            cx,
        );
        assert!(center.modal_visible());
        assert!(center.timer.is_none());
    });
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));
    center.update(visual, |center, cx| {
        center.dismiss_modal(cx);
        center.advance_time(Duration::ZERO, cx);
        assert!(!center.modal_visible());
    });
}

#[gpui_kit::test]
fn presentation_sound_waits_for_layout_and_never_repeats_on_route_changes(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (center, visual) = cx.add_window_view(|_, _| NotificationCenter::new());
    visual.simulate_resize(size(px(800.), px(700.)));
    let played = Rc::new(RefCell::new(Vec::new()));
    let capture = played.clone();
    let _subscription = visual.update(|_, cx| {
        cx.subscribe(
            &center,
            move |_, event: &super::NotificationPresented, _| {
                capture.borrow_mut().push(event.event_id)
            },
        )
    });
    let policy = NotificationSettings {
        sound: true,
        ..Default::default()
    };
    let appearance = NotificationAppearance {
        reduce_motion: true,
        ..Default::default()
    };
    let e = event();
    let id = e.id;
    center.update(visual, |center, cx| {
        center.configure(&policy, &appearance, environment(false), false, cx);
        center.enqueue(e, cx);
    });
    assert!(played.borrow().is_empty());
    center.update(visual, |center, cx| {
        center.configure(&policy, &appearance, environment(true), false, cx);
        assert!(
            played.borrow().is_empty(),
            "routing alone must not request sound"
        );
    });
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.run_until_parked();
    assert_eq!(*played.borrow(), [id]);
    center.update(visual, |center, cx| {
        center.configure(&policy, &appearance, environment(false), false, cx);
        center.configure(&policy, &appearance, environment(true), false, cx);
    });
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.run_until_parked();
    assert_eq!(*played.borrow(), [id]);
}

#[gpui_kit::test]
fn lower_severity_duplicate_cannot_downgrade_a_critical_modal_or_its_summary(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_kit::init);
    let (center, visual) = cx.add_window_view(|_, _| NotificationCenter::new());
    center.update(visual, |center, cx| {
        center.configure(
            &NotificationSettings::default(),
            &NotificationAppearance {
                reduce_motion: true,
                ..Default::default()
            },
            environment(true),
            false,
            cx,
        );
        let mut first = event();
        first.severity = NoticeSeverity::Critical;
        first.dedupe_key = Some("same-outcome".into());
        let id = first.id;
        let summary = first.summary.clone();
        center.enqueue(first, cx);
        let mut later = event();
        later.severity = NoticeSeverity::Information;
        later.summary = "普通状态".into();
        later.dedupe_key = Some("same-outcome".into());
        assert_eq!(center.enqueue(later, cx), Some(id));
        let entry = center.queue.visible().next().unwrap();
        assert_eq!(
            entry.route,
            studyapp_core::NoticeRoute::Foreground(NoticeStyle::ModalCard)
        );
        assert_eq!(entry.event.as_ref().unwrap().summary, summary);
        assert_eq!(entry.repetitions, 2);
        assert!(center.timer.is_none());
    });
}

#[gpui_kit::test]
fn mounted_control_anchors_move_clip_and_release_without_stale_ownership(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    struct Fixture {
        center: gpui_kit::Entity<NotificationCenter>,
        mounted: bool,
        x: f32,
    }
    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .relative()
                .children(self.mounted.then(|| {
                    crate::notification_anchor::NotificationAnchor::new(
                        "integrity-check",
                        self.center.clone(),
                        div()
                            .id("real-anchor-control")
                            .test_support()
                            .absolute()
                            .left(px(self.x))
                            .top(px(100.))
                            .w(px(160.))
                            .h(px(36.))
                            .child("检查文件完整性"),
                    )
                }))
                .child(self.center.clone())
        }
    }
    let mut center = None;
    let (fixture, visual) = cx.add_window_view(|_, cx| {
        let entity = cx.new(|_| NotificationCenter::new());
        center = Some(entity.clone());
        Fixture {
            center: entity,
            mounted: true,
            x: 80.,
        }
    });
    let center = center.unwrap();
    visual.simulate_resize(size(px(800.), px(700.)));
    let mut policy = NotificationSettings::default();
    policy.rules.insert(
        NoticeCategory::IntegrityResult,
        NoticeRule {
            style: Some(NoticeStyle::Anchored),
            ..Default::default()
        },
    );
    center.update(visual, |center, cx| {
        center.configure(
            &policy,
            &NotificationAppearance {
                reduce_motion: true,
                ..Default::default()
            },
            environment(true),
            false,
            cx,
        );
        let mut e = event();
        e.anchor = Some("integrity-check".into());
        center.enqueue(e, cx);
    });
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.run_until_parked();
    center.read_with(visual, |center, _| {
        assert_eq!(
            center.anchors["integrity-check"].origin,
            point(px(80.), px(100.))
        );
        assert_eq!(
            center.queue.visible().next().unwrap().route,
            studyapp_core::NoticeRoute::Foreground(NoticeStyle::Anchored)
        );
    });
    fixture.update(visual, |fixture, cx| {
        fixture.x = 180.;
        cx.notify();
    });
    visual.update(|window, cx| window.draw(cx).clear(cx));
    center.read_with(visual, |center, _| {
        assert_eq!(center.anchors["integrity-check"].origin.x, px(180.))
    });
    center.update(visual, |center, cx| {
        let old = center.anchor_owners["integrity-check"];
        let bounds = center.anchors["integrity-check"];
        center.publish_anchor("integrity-check", old + 1, bounds, cx);
        center.release_anchor("integrity-check", old, cx);
        assert!(center.anchors.contains_key("integrity-check"));
        // Return ownership to the mounted element for the following real unmount.
        center.publish_anchor("integrity-check", old, bounds, cx);
    });
    fixture.update(visual, |fixture, cx| {
        fixture.x = -1000.;
        cx.notify();
    });
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.run_until_parked();
    center.read_with(visual, |center, _| {
        assert!(!center.anchors.contains_key("integrity-check"));
        assert_eq!(
            center.queue.visible().next().unwrap().route,
            studyapp_core::NoticeRoute::Foreground(NoticeStyle::CornerCard)
        );
    });
    fixture.update(visual, |fixture, cx| {
        fixture.x = 80.;
        cx.notify();
    });
    visual.update(|window, cx| window.draw(cx).clear(cx));
    fixture.update(visual, |fixture, cx| {
        fixture.mounted = false;
        cx.notify();
    });
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.run_until_parked();
    center.read_with(visual, |center, _| {
        assert!(center.anchors.is_empty());
        assert_eq!(
            center.queue.visible().next().unwrap().route,
            studyapp_core::NoticeRoute::Foreground(NoticeStyle::CornerCard)
        );
    });
}
