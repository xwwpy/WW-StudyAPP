//! Bounded, non-blocking acknowledgement of a native window's requested level.
use std::time::{Duration, Instant};

const DEADLINE: Duration = Duration::from_secs(2);
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(80);

#[derive(Default)]
pub(crate) struct PinController {
    request: Option<(bool, u64)>,
    since: Option<Instant>,
    applied: bool,
    failed: bool,
}
pub(crate) enum PinUpdate {
    Idle,
    Pending,
    Applied,
    Failed(String),
}
impl PinController {
    pub(crate) fn applied(&self) -> bool {
        self.applied
    }
    pub(crate) fn pending(&self) -> bool {
        self.since.is_some()
    }
    pub(crate) fn failed(&self) -> bool {
        self.failed
    }

    /// `apply(true)` sends one request; later calls only observe its reply.
    /// `retry` permits an explicit retry of an unchanged saved preference.
    pub(crate) fn step(
        &mut self,
        target: bool,
        retry: u64,
        now: Instant,
        mut apply: impl FnMut(bool) -> anyhow::Result<bool>,
    ) -> PinUpdate {
        let first = self.request != Some((target, retry));
        if first {
            self.request = Some((target, retry));
            self.since = Some(now);
            self.failed = false;
        }
        let Some(since) = self.since else {
            return PinUpdate::Idle;
        };
        let result = if now.saturating_duration_since(since) >= DEADLINE {
            Err(anyhow::anyhow!("窗口管理器未在限定时间内确认置顶状态"))
        } else {
            apply(first)
        };
        match result {
            Ok(true) => {
                self.applied = target;
                self.since = None;
                PinUpdate::Applied
            }
            Ok(false) => PinUpdate::Pending,
            Err(error) => {
                self.since = None;
                self.failed = true;
                PinUpdate::Failed(format!("无法更新计时小组件置顶：{error:#}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sends_once_then_waits_for_ack_without_claiming_success() {
        let now = Instant::now();
        let mut pin = PinController::default();
        let mut calls = vec![];
        assert!(matches!(
            pin.step(true, 0, now, |first| {
                calls.push(first);
                Ok(false)
            }),
            PinUpdate::Pending
        ));
        assert!(!pin.applied());
        assert!(pin.pending());
        assert!(matches!(
            pin.step(true, 0, now + POLL_INTERVAL, |first| {
                calls.push(first);
                Ok(true)
            }),
            PinUpdate::Applied
        ));
        assert!(pin.applied());
        assert!(!pin.pending());
        assert!(matches!(
            pin.step(true, 0, now, |_| panic!("already acknowledged")),
            PinUpdate::Idle
        ));
        assert_eq!(calls, [true, false]);
    }
    #[test]
    fn timeout_is_reported_once_and_only_explicit_retry_restarts_it() {
        let now = Instant::now();
        let mut pin = PinController::default();
        assert!(matches!(
            pin.step(true, 0, now, |_| Ok(false)),
            PinUpdate::Pending
        ));
        assert!(matches!(
            pin.step(true, 0, now + DEADLINE, |_| panic!("deadline reached")),
            PinUpdate::Failed(_)
        ));
        assert!(pin.failed());
        assert!(!pin.applied());
        assert!(!pin.pending());
        assert!(matches!(
            pin.step(true, 0, now + DEADLINE, |_| panic!("no repeated failure")),
            PinUpdate::Idle
        ));
        assert!(matches!(
            pin.step(true, 1, now + DEADLINE, |first| {
                assert!(first);
                Ok(true)
            }),
            PinUpdate::Applied
        ));
        assert!(pin.applied());
        assert!(!pin.failed());
    }
    #[test]
    fn changing_target_cancels_pending_or_failed_requests_and_keeps_last_applied_level() {
        let now = Instant::now();
        let mut pin = PinController::default();
        assert!(matches!(
            pin.step(true, 0, now, |_| Ok(true)),
            PinUpdate::Applied
        ));
        assert!(matches!(
            pin.step(false, 0, now, |_| anyhow::bail!("unsupported WM")),
            PinUpdate::Failed(_)
        ));
        assert!(
            pin.applied(),
            "failure cannot pretend that unpinning succeeded"
        );
        assert!(matches!(
            pin.step(true, 0, now, |first| {
                assert!(first);
                Ok(true)
            }),
            PinUpdate::Applied
        ));
        assert!(pin.applied());
        assert!(!pin.failed());
    }
}
