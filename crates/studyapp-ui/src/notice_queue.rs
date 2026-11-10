//! Pure notification lifecycle; timestamps are supplied by the view or tests.
use std::{collections::VecDeque, time::Duration};
use studyapp_core::{
    NoticeEvent, NoticeEventId, NoticeRoute, NoticeSeverity, NoticeStyle, NotificationAppearance,
};

const VISIBLE_LIMIT: usize = 2;
const PENDING_LIMIT: usize = 8;
const SUCCESS_LIFETIME: Duration = Duration::from_secs(4);
const ERROR_LIFETIME: Duration = Duration::from_secs(8);
const TRANSITION: Duration = Duration::from_millis(200);

#[derive(Clone)]
pub struct Notice {
    pub id: u64,
    pub message: Result<String, String>,
    pub event: Option<NoticeEvent>,
    pub route: NoticeRoute,
    pub repetitions: u32,
    persistent: bool,
    presented: bool,
    animated: bool,
    shown_at: Option<Duration>,
    closing_at: Option<Duration>,
    closing_opacity: f32,
    remaining: Duration,
    hovered: bool,
}

impl Notice {
    #[cfg(feature = "ui-test")]
    pub fn is_closing(&self) -> bool {
        self.closing_at.is_some()
    }

    pub fn opacity(&self, now: Duration) -> f32 {
        if !self.animated {
            return if self.closing_at.is_some() { 0. } else { 1. };
        }
        if let Some(start) = self.closing_at {
            self.closing_opacity
                * (1.
                    - (now.saturating_sub(start).as_secs_f32() / TRANSITION.as_secs_f32()).min(1.))
        } else {
            self.shown_at.map_or(0., |start| {
                (now.saturating_sub(start).as_secs_f32() / TRANSITION.as_secs_f32()).min(1.)
            })
        }
    }
}

pub struct NoticeQueue {
    entries: VecDeque<Notice>,
    next_id: u64,
    last_tick: Duration,
    visible_limit: usize,
}
impl Default for NoticeQueue {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            next_id: 0,
            last_tick: Duration::ZERO,
            visible_limit: VISIBLE_LIMIT,
        }
    }
}

impl NoticeQueue {
    pub fn push(&mut self, message: Result<String, String>, now: Duration) -> u64 {
        self.tick(now);
        self.next_id += 1;
        let remaining = if message.is_ok() {
            SUCCESS_LIFETIME
        } else {
            ERROR_LIFETIME
        };
        self.entries.push_back(Notice {
            id: self.next_id,
            message,
            event: None,
            route: NoticeRoute::Foreground(NoticeStyle::Bubble),
            repetitions: 1,
            persistent: false,
            presented: false,
            animated: true,
            shown_at: None,
            closing_at: None,
            closing_opacity: 1.,
            remaining,
            hovered: false,
        });
        self.promote(now);
        self.bound_pending();
        self.next_id
    }

    pub fn push_event(
        &mut self,
        event: NoticeEvent,
        route: NoticeRoute,
        appearance: &NotificationAppearance,
        now: Duration,
    ) -> Option<NoticeEventId> {
        if route == NoticeRoute::Suppressed
            || event.validate().is_err()
            || appearance.validate().is_err()
        {
            return None;
        }
        self.tick(now);
        self.visible_limit = usize::from(appearance.visible_limit());
        if let Some(key) = &event.dedupe_key
            && let Some(index) = self.entries.iter().position(|entry| {
                entry.closing_at.is_none()
                    && entry.event.as_ref().is_some_and(|previous| {
                        previous.category == event.category
                            && previous.dedupe_key.as_ref() == Some(key)
                    })
            })
        {
            let entry = &mut self.entries[index];
            let previous = entry.event.as_mut().expect("matched event");
            if event.severity >= previous.severity {
                previous.severity = event.severity;
                previous.summary = event.summary;
                previous.action = event.action.or(previous.action);
                previous.anchor = event.anchor.or(previous.anchor.take());
            }
            entry.message = if previous.severity >= NoticeSeverity::Warning {
                Err(previous.summary.clone())
            } else {
                Ok(previous.summary.clone())
            };
            entry.persistent = previous.severity.persistent();
            entry.repetitions = entry.repetitions.saturating_add(1);
            entry.remaining = Duration::from_secs(u64::from(appearance.duration_seconds));
            let id = previous.id;
            if entry.route != route {
                entry.route = route;
                entry.shown_at = None;
                entry.hovered = false;
            }
            if previous.severity == NoticeSeverity::Critical {
                let entry = self.entries.remove(index).expect("matched event index");
                self.entries.push_front(entry);
            }
            self.promote(now);
            return Some(id);
        }
        self.next_id = self.next_id.saturating_add(1);
        let event_id = event.id;
        let severity = event.severity;
        let entry = Notice {
            id: self.next_id,
            message: if severity >= NoticeSeverity::Warning {
                Err(event.summary.clone())
            } else {
                Ok(event.summary.clone())
            },
            event: Some(event),
            route,
            repetitions: 1,
            persistent: severity.persistent(),
            presented: false,
            animated: appearance.animation && !appearance.reduce_motion,
            shown_at: None,
            closing_at: None,
            closing_opacity: 1.,
            remaining: Duration::from_secs(u64::from(appearance.duration_seconds)),
            hovered: false,
        };
        if severity == NoticeSeverity::Critical {
            self.entries.push_front(entry);
        } else {
            self.entries.push_back(entry);
        }
        self.promote(now);
        self.bound_pending();
        Some(event_id)
    }

    pub fn reroute(
        &mut self,
        settings: &studyapp_core::NotificationSettings,
        appearance: &NotificationAppearance,
        environment: studyapp_core::NoticeEnvironment,
        now: Duration,
    ) {
        self.reroute_with(appearance, now, |event| {
            settings.route(event.category, event.severity, appearance, environment)
        });
    }

    pub fn reroute_with(
        &mut self,
        appearance: &NotificationAppearance,
        now: Duration,
        mut resolve: impl FnMut(&NoticeEvent) -> NoticeRoute,
    ) {
        self.tick(now);
        self.visible_limit = usize::from(appearance.visible_limit().clamp(1, 4));
        for entry in &mut self.entries {
            if let Some(event) = &entry.event {
                let next = resolve(event);
                if entry.route != next {
                    entry.hovered = false;
                    entry.shown_at = None;
                    entry.remaining = Duration::from_secs(u64::from(appearance.duration_seconds));
                }
                entry.animated = appearance.animation && !appearance.reduce_motion;
                entry.route = next;
            }
        }
        self.entries
            .retain(|entry| entry.route != NoticeRoute::Suppressed);
        self.promote(now);
        self.bound_pending();
    }

    pub fn visible(&self) -> impl Iterator<Item = &Notice> {
        let mut modal_selected = false;
        self.entries
            .iter()
            .filter(move |entry| match entry.route {
                NoticeRoute::Foreground(NoticeStyle::ModalCard) => {
                    !std::mem::replace(&mut modal_selected, true)
                }
                NoticeRoute::Foreground(_) => true,
                _ => false,
            })
            .take(self.visible_limit)
    }
    pub fn desktop_visible(&self) -> impl Iterator<Item = &Notice> {
        self.entries
            .iter()
            .filter(|entry| entry.route == NoticeRoute::DesktopCard)
            .take(self.visible_limit)
    }
    /// A delivery is acknowledged by a laid-out surface, never by enqueue/routing.
    pub fn did_present(&mut self, ids: &[u64], route: NoticeRoute) -> Vec<NoticeEventId> {
        let active = self.active_ids();
        self.entries
            .iter_mut()
            .filter_map(|entry| {
                if !entry.presented
                    && entry.closing_at.is_none()
                    && entry.route == route
                    && ids.contains(&entry.id)
                    && active.contains(&entry.id)
                {
                    entry.presented = true;
                    entry.event.as_ref().map(|event| event.id)
                } else {
                    None
                }
            })
            .collect()
    }

    fn active_ids(&self) -> Vec<u64> {
        self.visible()
            .chain(self.desktop_visible())
            .map(|entry| entry.id)
            .collect()
    }
    fn bound_pending(&mut self) {
        loop {
            let active = self.active_ids();
            if self.entries.len() <= active.len().max(self.visible_limit) + PENDING_LIMIT {
                break;
            }
            let index = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, entry)| !active.contains(&entry.id))
                .min_by_key(|(_, entry)| {
                    entry
                        .event
                        .as_ref()
                        .map(|event| event.severity)
                        .unwrap_or(NoticeSeverity::Information)
                })
                .map(|(index, _)| index);
            if let Some(index) = index {
                self.entries.remove(index);
            } else {
                break;
            }
        }
    }

    pub fn routes(&self) -> impl Iterator<Item = (studyapp_core::NoticeEventId, NoticeRoute)> + '_ {
        self.entries
            .iter()
            .filter_map(|entry| entry.event.as_ref().map(|event| (event.id, entry.route)))
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn acknowledge(&mut self, id: u64, now: Duration) {
        self.tick(now);
        self.entries.retain(|entry| entry.id != id);
        self.promote(now);
    }

    pub fn dismiss(&mut self, id: u64, now: Duration) {
        self.tick(now);
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id)
            && entry.closing_at.is_none()
        {
            entry.closing_opacity = entry.opacity(now);
            entry.closing_at = Some(now);
        }
    }

    pub fn hover(&mut self, id: u64, hovered: bool, now: Duration) {
        self.tick(now);
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) {
            entry.hovered = hovered;
        }
    }

    fn promote(&mut self, now: Duration) {
        let ids = self.active_ids();
        for entry in &mut self.entries {
            if ids.contains(&entry.id) {
                entry.shown_at.get_or_insert(now);
            }
        }
    }

    pub fn animating(&self, now: Duration) -> bool {
        self.visible()
            .chain(self.desktop_visible())
            .any(|entry| entry.closing_at.is_some() || entry.opacity(now) < 1.)
    }

    pub fn needs_timer(&self, now: Duration) -> bool {
        self.animating(now)
            || self
                .visible()
                .chain(self.desktop_visible())
                .any(|entry| !entry.persistent && entry.closing_at.is_none())
    }

    /// Only animation/structural transitions require a frame, not timer ticks.
    pub fn tick(&mut self, now: Duration) -> bool {
        let elapsed = now.saturating_sub(self.last_tick);
        let mut changed = self.animating(self.last_tick);
        self.last_tick = now;
        let ids = self.active_ids();
        for entry in &mut self.entries {
            if ids.contains(&entry.id)
                && entry.shown_at.is_some()
                && entry.closing_at.is_none()
                && !entry.hovered
                && !entry.persistent
            {
                entry.remaining = entry.remaining.saturating_sub(elapsed);
                if entry.remaining.is_zero() {
                    entry.closing_at = Some(now);
                    changed = true;
                }
            }
        }
        let before = self.entries.len();
        self.entries.retain(|entry| {
            entry
                .closing_at
                .is_none_or(|at| entry.animated && now.saturating_sub(at) < TRANSITION)
        });
        changed |= before != self.entries.len();
        self.promote(now);
        changed || self.animating(now)
    }

    #[cfg(feature = "ui-test")]
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    #[test]
    fn sustained_duplicate_stream_bounds_backlog_and_promotes_recent_events_with_fresh_lifetime() {
        let mut queue = NoticeQueue::default();
        let first = queue.push(Err("storage unavailable".into()), ms(0));
        let second = queue.push(Err("storage unavailable".into()), ms(0));
        queue.hover(first, true, ms(0));
        queue.hover(second, true, ms(0));
        let mut arrivals = Vec::new();
        for elapsed_seconds in 1..=100 {
            arrivals.push(queue.push(
                Err("storage unavailable".into()),
                ms(elapsed_seconds * 1000),
            ));
            assert!(
                queue.entries.len() <= 10,
                "Two visible plus at most eight waiting notices"
            );
            assert_eq!(
                queue.visible().map(|n| n.id).collect::<Vec<_>>(),
                [first, second]
            );
        }
        let waiting = queue.entries.iter().skip(2).collect::<Vec<_>>();
        assert_eq!(
            waiting.iter().map(|n| n.id).collect::<Vec<_>>(),
            arrivals[arrivals.len() - 8..]
        );
        assert!(
            waiting
                .iter()
                .all(|n| n.shown_at.is_none() && n.remaining == ERROR_LIFETIME)
        );
        let promoted = waiting[0].id;
        queue.dismiss(first, ms(100000));
        queue.tick(ms(100200));
        let entry = queue.visible().find(|n| n.id == promoted).unwrap();
        assert_eq!(entry.shown_at, Some(ms(100200)));
        assert_eq!(entry.remaining, ERROR_LIFETIME);
        queue.tick(ms(108199));
        let entry = queue.visible().find(|n| n.id == promoted).unwrap();
        assert_eq!(entry.remaining, ms(1));
        assert!(entry.closing_at.is_none());
    }

    #[test]
    fn identical_events_have_independent_ids_and_visible_slots() {
        let mut queue = NoticeQueue::default();
        let a = queue.push(Ok("saved".into()), ms(0));
        let b = queue.push(Ok("saved".into()), ms(0));
        assert_ne!(a, b);
        assert_eq!(queue.visible().count(), 2);
    }

    #[test]
    fn dismiss_during_entrance_fades_from_current_opacity_without_flash() {
        let mut queue = NoticeQueue::default();
        let id = queue.push(Ok("saved".into()), ms(0));
        let before = queue.visible().next().unwrap().opacity(ms(50));
        queue.dismiss(id, ms(50));
        assert_eq!(queue.visible().next().unwrap().opacity(ms(50)), before);
        assert!(queue.visible().next().unwrap().opacity(ms(100)) < before);
    }

    #[test]
    fn dismiss_animates_then_promotes_queued_message_with_fresh_lifetime() {
        let mut queue = NoticeQueue::default();
        let first = queue.push(Ok("one".into()), ms(0));
        queue.push(Ok("two".into()), ms(0));
        let third = queue.push(Ok("three".into()), ms(0));
        queue.dismiss(first, ms(500));
        queue.tick(ms(600));
        assert!(
            queue
                .visible()
                .any(|n| n.id == first && n.opacity(ms(600)) > 0. && n.opacity(ms(600)) < 1.)
        );
        assert!(!queue.visible().any(|n| n.id == third));
        queue.tick(ms(700));
        assert!(!queue.visible().any(|n| n.id == first));
        assert!(
            queue
                .visible()
                .any(|n| n.id == third && n.remaining == SUCCESS_LIFETIME)
        );
    }

    #[test]
    fn hover_pauses_expiry_and_success_fades_without_manual_dismissal() {
        let mut queue = NoticeQueue::default();
        let id = queue.push(Ok("saved".into()), ms(0));
        queue.hover(id, true, ms(1000));
        queue.tick(ms(10000));
        assert_eq!(queue.visible().next().unwrap().remaining, ms(3000));
        queue.hover(id, false, ms(10000));
        queue.tick(ms(13000));
        assert_eq!(queue.visible().count(), 1);
        queue.tick(ms(13200));
        assert!(queue.is_empty());
    }

    #[test]
    fn error_is_readable_longer_and_idle_frames_do_not_require_redraw() {
        let mut queue = NoticeQueue::default();
        queue.push(Err("failure".into()), ms(0));
        queue.tick(ms(200));
        assert!(!queue.tick(ms(300)));
        queue.tick(ms(4200));
        assert_eq!(queue.visible().count(), 1);
        queue.tick(ms(8000));
        assert!(queue.animating(ms(8050)));
        queue.tick(ms(8200));
        assert!(queue.is_empty());
    }
}

#[cfg(test)]
mod structured_tests {
    use super::NoticeQueue;
    use std::time::Duration;
    use studyapp_core::{
        NoticeCategory as Category, NoticeEnvironment, NoticeEvent, NoticeRoute as Route,
        NoticeSeverity as Severity, NoticeStyle as Style, NotificationAppearance,
        NotificationSettings,
    };
    fn at(seconds: u64) -> Duration {
        Duration::from_secs(seconds)
    }
    fn event(key: &str, severity: Severity) -> NoticeEvent {
        let mut event = NoticeEvent::new(Category::SaveFailure, severity, "无法保存，请重试");
        event.dedupe_key = Some(key.into());
        event
    }
    #[test]
    fn single_card_mode_keeps_other_types_queued_until_dismissal() {
        let mut queue = NoticeQueue::default();
        let appearance = NotificationAppearance {
            stack: studyapp_core::NoticeStack::Queue,
            max_visible: 4,
            ..Default::default()
        };
        let first = event("first", Severity::Error);
        let first_event_id = first.id;
        queue.push_event(
            first,
            Route::Foreground(Style::CornerCard),
            &appearance,
            at(0),
        );
        let second = event("second", Severity::Error);
        let second_event_id = second.id;
        queue.push_event(
            second,
            Route::Foreground(Style::ActionSheet),
            &appearance,
            at(0),
        );
        assert_eq!(queue.visible().count(), 1);
        let id = queue.visible().next().unwrap().id;
        assert_eq!(
            queue.visible().next().unwrap().event.as_ref().unwrap().id,
            first_event_id
        );
        queue.tick(at(1000));
        assert_eq!(queue.visible().count(), 1);
        queue.dismiss(id, at(1000));
        queue.tick(at(1001));
        assert_eq!(queue.visible().count(), 1);
        let promoted = queue.visible().next().unwrap();
        assert_eq!(promoted.event.as_ref().unwrap().id, second_event_id);
        assert_eq!(promoted.route, Route::Foreground(Style::ActionSheet));
    }
    #[test]
    fn explicit_dedupe_keeps_one_correlation_id_and_errors_do_not_expire() {
        let mut queue = NoticeQueue::default();
        let appearance = NotificationAppearance::default();
        let first = event("draft-a", Severity::Error);
        let id = first.id;
        assert_eq!(
            queue.push_event(
                first,
                Route::Foreground(Style::CornerCard),
                &appearance,
                at(0)
            ),
            Some(id)
        );
        for second in 1..20 {
            assert_eq!(
                queue.push_event(
                    event("draft-a", Severity::Error),
                    Route::Foreground(Style::CornerCard),
                    &appearance,
                    at(second)
                ),
                Some(id)
            );
        }
        queue.tick(at(86400));
        assert!(!queue.needs_timer(at(86400)));
        assert_eq!(queue.visible().count(), 1);
        let entry = queue.visible().next().unwrap();
        assert_eq!(entry.repetitions, 20);
        assert_eq!(entry.event.as_ref().unwrap().id, id);
        assert!(entry.closing_at.is_none());
        let queue_id = entry.id;
        queue.dismiss(queue_id, at(86400));
        queue.tick(at(86401));
        assert!(queue.is_empty());
    }
    #[test]
    fn background_without_floating_window_is_retained_and_moves_to_foreground_once() {
        let mut queue = NoticeQueue::default();
        let appearance = NotificationAppearance::default();
        let event = event("pending", Severity::Error);
        let id = event.id;
        queue.push_event(event, Route::Deferred, &appearance, at(0));
        queue.tick(at(3600));
        assert!(!queue.is_empty());
        assert!(!queue.needs_timer(at(3600)));
        assert_eq!(queue.visible().count(), 0);
        assert_eq!(queue.desktop_visible().count(), 0);
        let environment = NoticeEnvironment {
            foreground: true,
            desktop_available: false,
            anchor_available: false,
            minute: 720,
        };
        queue.reroute(
            &NotificationSettings::default(),
            &appearance,
            environment,
            at(3600),
        );
        assert_eq!(
            queue.visible().next().unwrap().event.as_ref().unwrap().id,
            id
        );
        queue.reroute(
            &NotificationSettings::default(),
            &appearance,
            environment,
            at(3601),
        );
        assert_eq!(queue.visible().count(), 1);
        assert_eq!(queue.desktop_visible().count(), 0);
    }
    #[test]
    fn desktop_and_foreground_are_exclusive_for_the_same_event_and_backlogs_stay_bounded() {
        let mut queue = NoticeQueue::default();
        let appearance = NotificationAppearance::default();
        for n in 0..100 {
            queue.push_event(
                event(&format!("item-{n}"), Severity::Error),
                Route::DesktopCard,
                &appearance,
                at(n),
            );
        }
        assert!(queue.entries.len() <= 10);
        assert_eq!(queue.visible().count(), 0);
        assert_eq!(queue.desktop_visible().count(), 2);
        queue.reroute(
            &NotificationSettings::default(),
            &appearance,
            NoticeEnvironment {
                foreground: true,
                desktop_available: true,
                anchor_available: false,
                minute: 720,
            },
            at(100),
        );
        assert_eq!(queue.visible().count(), 2);
        assert_eq!(queue.desktop_visible().count(), 0);
    }
    #[test]
    fn duplicate_arrival_promotes_deferred_event_without_changing_its_uuid() {
        let mut queue = NoticeQueue::default();
        let appearance = NotificationAppearance {
            reduce_motion: true,
            ..Default::default()
        };
        let first = event("same-operation", Severity::Information);
        let id = first.id;
        queue.push_event(first, Route::Deferred, &appearance, at(0));
        assert!(!queue.needs_timer(at(400)));
        assert_eq!(
            queue.push_event(
                event("same-operation", Severity::Information),
                Route::Foreground(Style::Bubble),
                &appearance,
                at(400)
            ),
            Some(id)
        );
        let entry = queue.visible().next().unwrap();
        assert_eq!(entry.event.as_ref().unwrap().id, id);
        assert_eq!(entry.repetitions, 2);
        assert_eq!(entry.shown_at, Some(at(400)));
        assert!(queue.needs_timer(at(400)));
        queue.tick(at(406));
        assert!(queue.is_empty());
    }

    #[test]
    fn critical_duplicate_preempts_full_slots_and_route_change_clears_hover() {
        let mut queue = NoticeQueue::default();
        let appearance = NotificationAppearance {
            reduce_motion: true,
            ..Default::default()
        };
        for key in ["first", "second", "escalates"] {
            queue.push_event(
                event(key, Severity::Information),
                Route::Foreground(Style::Bubble),
                &appearance,
                at(0),
            );
        }
        let original_id = queue.entries.back().unwrap().event.as_ref().unwrap().id;
        queue.push_event(
            event("escalates", Severity::Critical),
            Route::Foreground(Style::ModalCard),
            &appearance,
            at(0),
        );
        let first = queue.visible().next().unwrap();
        assert_eq!(first.event.as_ref().unwrap().id, original_id);
        let id = first.id;
        queue.hover(id, true, at(0));
        queue.push_event(
            event("escalates", Severity::Critical),
            Route::DesktopCard,
            &appearance,
            at(0),
        );
        assert!(!queue.desktop_visible().next().unwrap().hovered);
    }

    #[test]
    fn critical_preempts_regular_slots_and_reduced_motion_dismisses_without_a_transition() {
        let mut queue = NoticeQueue::default();
        let appearance = NotificationAppearance {
            reduce_motion: true,
            ..Default::default()
        };
        for key in ["a", "b"] {
            queue.push_event(
                event(key, Severity::Error),
                Route::Foreground(Style::CornerCard),
                &appearance,
                at(0),
            );
        }
        let critical = event("critical", Severity::Critical);
        let id = critical.id;
        queue.push_event(
            critical,
            Route::Foreground(Style::ModalCard),
            &appearance,
            at(0),
        );
        let first = queue.visible().next().unwrap();
        assert_eq!(first.event.as_ref().unwrap().id, id);
        assert_eq!(first.opacity(at(0)), 1.);
        queue.dismiss(first.id, at(0));
        queue.tick(at(0));
        assert!(
            !queue
                .visible()
                .any(|entry| entry.event.as_ref().unwrap().id == id)
        );
    }
    #[test]
    fn queued_modal_does_not_consume_lifetime_before_its_own_slot() {
        let mut queue = NoticeQueue::default();
        let appearance = NotificationAppearance {
            reduce_motion: true,
            max_visible: 4,
            ..Default::default()
        };
        for key in ["modal-first", "modal-second"] {
            queue.push_event(
                event(key, Severity::Information),
                Route::Foreground(Style::ModalCard),
                &appearance,
                at(0),
            );
        }
        assert_eq!(queue.visible().count(), 1);
        let second = queue.entries[1].id;
        assert!(
            queue
                .did_present(&[second], Route::Foreground(Style::ModalCard))
                .is_empty()
        );
        queue.tick(at(4));
        assert_eq!(queue.entries[1].remaining, at(5));
        assert!(queue.entries[1].shown_at.is_none());
        queue.tick(at(5));
        assert_eq!(queue.visible().next().unwrap().id, second);
        assert_eq!(queue.visible().next().unwrap().remaining, at(5));
        assert_eq!(
            queue
                .did_present(&[second], Route::Foreground(Style::ModalCard))
                .len(),
            1
        );
        assert!(
            queue
                .did_present(&[second], Route::Foreground(Style::ModalCard))
                .is_empty()
        );
        queue.tick(at(9));
        assert_eq!(queue.visible().next().unwrap().remaining, at(1));
        queue.tick(at(10));
        assert!(queue.is_empty());
    }

    #[test]
    fn presentation_is_once_per_event_and_never_for_deferred_or_unshown_entries() {
        let mut queue = NoticeQueue::default();
        let appearance = NotificationAppearance {
            reduce_motion: true,
            max_visible: 1,
            ..Default::default()
        };
        let first = event("deferred", Severity::Error);
        let id = first.id;
        queue.push_event(first, Route::Deferred, &appearance, at(0));
        let numeric_id = queue.entries[0].id;
        assert!(
            queue
                .did_present(&[numeric_id], Route::DesktopCard)
                .is_empty()
        );
        queue.reroute_with(&appearance, at(10), |_| Route::DesktopCard);
        assert!(
            queue
                .did_present(&[numeric_id], Route::Foreground(Style::CornerCard))
                .is_empty()
        );
        assert_eq!(queue.did_present(&[numeric_id], Route::DesktopCard), [id]);
        queue.reroute_with(&appearance, at(20), |_| {
            Route::Foreground(Style::CornerCard)
        });
        assert!(
            queue
                .did_present(&[numeric_id], Route::Foreground(Style::CornerCard))
                .is_empty()
        );
    }
}
