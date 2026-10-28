//! External drag feedback contains no paths and performs no filesystem work.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropBlock {
    Busy,
    Modal,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExternalDropState {
    item_count: usize,
    blocked: Option<DropBlock>,
}

impl ExternalDropState {
    /// Only the platform's external-path drag should call this. Repeated moves
    /// with the same payload are no-ops, avoiding a native visibility churn.
    pub fn hover(&mut self, item_count: usize, blocked: Option<DropBlock>) -> bool {
        let next = if item_count == 0 {
            Self::default()
        } else {
            Self {
                item_count,
                blocked,
            }
        };
        let changed = *self != next;
        *self = next;
        changed
    }

    pub fn active(&self) -> bool {
        self.item_count != 0
    }

    pub fn item_count(&self) -> usize {
        self.item_count
    }

    pub fn blocked(&self) -> Option<DropBlock> {
        self.blocked
    }

    pub fn set_blocked(&mut self, blocked: Option<DropBlock>) -> bool {
        if !self.active() || self.blocked == blocked {
            return false;
        }
        self.blocked = blocked;
        true
    }

    /// Both leave and Escape/cancel use this; restoring an inactive surface is
    /// harmless and does not require a render.
    pub fn cancel(&mut self) -> bool {
        let changed = self.active();
        *self = Self::default();
        changed
    }

    /// Recheck the host's current guards on release, then consume the hover.
    /// Enumeration and ingestion happen in the host worker after this returns.
    pub fn finish(&mut self, blocked: Option<DropBlock>) -> bool {
        let accept = self.active() && blocked.is_none();
        self.cancel();
        accept
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_hover_is_idempotent_and_leave_restores_the_surface() {
        let mut state = ExternalDropState::default();
        assert!(state.hover(2, None));
        assert!(state.active());
        assert!(!state.hover(2, None));
        assert_eq!(state.item_count(), 2);
        assert!(state.cancel());
        assert!(!state.active());
        assert!(!state.cancel());
    }

    #[test]
    fn empty_and_internal_drags_never_activate_import() {
        let mut state = ExternalDropState::default();
        assert!(!state.hover(0, None));
        assert!(!state.active());
        // Internal note/folder drag handlers never call hover; clearing their
        // predecessor prevents a stale external overlay from leaking through.
        state.hover(1, None);
        state.cancel();
        assert!(!state.active());
    }

    #[test]
    fn busy_or_modal_hover_is_visible_but_cannot_be_accepted() {
        for reason in [DropBlock::Busy, DropBlock::Modal] {
            let mut state = ExternalDropState::default();
            state.hover(1, Some(reason));
            assert!(state.active());
            assert_eq!(state.blocked(), Some(reason));
            assert!(!state.finish(Some(reason)));
            assert!(!state.active());
        }
    }

    #[test]
    fn release_rechecks_latest_guard_and_consumes_the_hover_once() {
        let mut state = ExternalDropState::default();
        state.hover(3, None);
        assert!(!state.finish(Some(DropBlock::Busy)));
        assert!(!state.finish(None));
        state.hover(3, None);
        assert!(state.finish(None));
        assert!(!state.finish(None));
    }

    #[test]
    fn guard_changes_update_feedback_without_changing_item_count() {
        let mut state = ExternalDropState::default();
        state.hover(4, None);
        assert!(state.set_blocked(Some(DropBlock::Modal)));
        assert_eq!(state.item_count(), 4);
        assert_eq!(state.blocked(), Some(DropBlock::Modal));
        assert!(!state.set_blocked(Some(DropBlock::Modal)));
        state.cancel();
        assert!(!state.set_blocked(Some(DropBlock::Busy)));
    }
}
