//! Pure notification lifecycle; timestamps are supplied by the view or tests.
use std::{collections::VecDeque, time::Duration};

const VISIBLE_LIMIT: usize = 2;
const PENDING_LIMIT: usize = 8;
const SUCCESS_LIFETIME: Duration = Duration::from_secs(4);
const ERROR_LIFETIME: Duration = Duration::from_secs(8);
const TRANSITION: Duration = Duration::from_millis(200);

pub struct Notice {
    pub id: u64,
    pub message: Result<String, String>,
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

#[derive(Default)]
pub struct NoticeQueue {
    entries: VecDeque<Notice>,
    next_id: u64,
    last_tick: Duration,
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
            shown_at: None,
            closing_at: None,
            closing_opacity: 1.,
            remaining,
            hovered: false,
        });
        self.promote(now);
        // Background failures may repeat indefinitely. Preserve the visible
        // pair and distinct recent events while discarding stale waiting work.
        if self.entries.len() > VISIBLE_LIMIT + PENDING_LIMIT {
            let _ = self.entries.remove(VISIBLE_LIMIT);
        }
        self.next_id
    }

    pub fn visible(&self) -> impl Iterator<Item = &Notice> {
        self.entries.iter().take(VISIBLE_LIMIT)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
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
        for entry in self.entries.iter_mut().take(VISIBLE_LIMIT) {
            entry.shown_at.get_or_insert(now);
        }
    }

    pub fn animating(&self, now: Duration) -> bool {
        self.visible()
            .any(|entry| entry.closing_at.is_some() || entry.opacity(now) < 1.)
    }

    /// Only animation/structural transitions require a frame, not timer ticks.
    pub fn tick(&mut self, now: Duration) -> bool {
        let elapsed = now.saturating_sub(self.last_tick);
        let mut changed = self.animating(self.last_tick);
        self.last_tick = now;
        for entry in self.entries.iter_mut().take(VISIBLE_LIMIT) {
            if entry.shown_at.is_some() && entry.closing_at.is_none() && !entry.hovered {
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
                .is_none_or(|at| now.saturating_sub(at) < TRANSITION)
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
