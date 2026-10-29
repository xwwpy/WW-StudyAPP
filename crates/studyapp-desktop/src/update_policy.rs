/// A manual request already in flight also satisfies the single startup attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckOrigin {
    Startup,
    Manual,
    Periodic,
}

#[derive(Default)]
pub struct UpdateCheckPolicy {
    startup_claimed: bool,
    in_flight: bool,
    next_due: Option<u64>,
    dismissed: std::collections::HashSet<String>,
}

impl UpdateCheckPolicy {
    /// Seconds since the panel's monotonic epoch; busy periods coalesce into one check.
    pub fn periodic_due(&mut self, now: u64, busy: bool) -> bool {
        if busy || self.in_flight || self.next_due.is_some_and(|due| now < due) {
            return false;
        }
        self.next_due = Some(now.saturating_add(3600));
        true
    }

    pub fn dismiss(&mut self, version: &str) {
        self.dismissed.insert(version.to_owned());
    }

    pub fn should_prompt(&self, origin: CheckOrigin, version: &str) -> bool {
        origin == CheckOrigin::Manual || !self.dismissed.contains(version)
    }
    pub fn start(&mut self, origin: CheckOrigin) -> bool {
        if origin == CheckOrigin::Startup {
            if self.startup_claimed {
                return false;
            }
            self.startup_claimed = true;
        }
        if self.in_flight {
            return false;
        }
        self.in_flight = true;
        true
    }

    pub fn finish(&mut self) {
        self.in_flight = false;
    }
}

/// Isolated fixtures and maintenance entrypoints never contact release services.
pub fn startup_check_allowed<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut args = args.into_iter();
    if args.next().is_some_and(|exe| {
        exe.as_ref()
            .to_string_lossy()
            .rsplit(['/', '\\'])
            .next()
            .is_some_and(|name| name.eq_ignore_ascii_case("Uninstall.exe"))
    }) {
        return false;
    }
    !args.any(|arg| {
        let arg = arg.as_ref().to_string_lossy();
        arg == "--data-dir"
            || arg.starts_with("--data-dir=")
            || arg == "--smoke-test"
            || arg == "--uninstall"
            || arg.starts_with("--maintenance-")
    })
}

#[cfg(test)]
mod tests {
    use super::{CheckOrigin, UpdateCheckPolicy, startup_check_allowed};

    #[test]
    fn startup_check_occurs_once_and_manual_check_coalesces() {
        let mut policy = UpdateCheckPolicy::default();
        assert!(policy.start(CheckOrigin::Startup));
        assert!(!policy.start(CheckOrigin::Manual));
        policy.finish();
        assert!(!policy.start(CheckOrigin::Startup));
        assert!(policy.start(CheckOrigin::Manual));
    }

    #[test]
    fn manual_check_before_startup_satisfies_startup_attempt() {
        let mut policy = UpdateCheckPolicy::default();
        assert!(policy.start(CheckOrigin::Manual));
        assert!(!policy.start(CheckOrigin::Startup));
        policy.finish();
        assert!(!policy.start(CheckOrigin::Startup));
        assert!(policy.start(CheckOrigin::Manual));
        policy.finish();
        assert!(policy.start(CheckOrigin::Manual));
    }

    #[test]
    fn update_startup_skips_isolated_and_maintenance_launches() {
        assert!(startup_check_allowed(["studyapp.exe"]));
        for args in [
            vec!["studyapp.exe", "--data-dir", "C:/fixture"],
            vec!["studyapp.exe", "--data-dir=C:/fixture"],
            vec!["studyapp.exe", "--smoke-test"],
            vec!["studyapp.exe", "--uninstall"],
            vec!["Uninstall.exe"],
            vec!["C:/fixture/UNINSTALL.EXE"],
            vec!["studyapp.exe", "--maintenance-apply", "plan"],
        ] {
            assert!(!startup_check_allowed(args));
        }
    }
}

#[cfg(test)]
mod periodic_tests {
    use super::*;
    #[test]
    fn all_dismissed_versions_remain_quiet_during_session() {
        let mut policy = UpdateCheckPolicy::default();
        policy.dismiss("0.9.0");
        policy.dismiss("0.9.1");
        assert!(!policy.should_prompt(CheckOrigin::Periodic, "0.9.0"));
        assert!(!policy.should_prompt(CheckOrigin::Periodic, "0.9.1"));
        assert!(policy.should_prompt(CheckOrigin::Manual, "0.9.0"));
        assert!(policy.should_prompt(CheckOrigin::Periodic, "0.9.2"));
    }
    #[test]
    fn startup_completion_allows_hourly_retry_without_startup_repetition() {
        let mut policy = UpdateCheckPolicy::default();
        assert!(policy.periodic_due(0, false));
        assert!(policy.start(CheckOrigin::Startup));
        policy.finish(); // Failure and success both release the same check claim.
        assert!(!policy.start(CheckOrigin::Startup));
        assert!(!policy.periodic_due(3599, false));
        assert!(policy.periodic_due(3600, false));
        assert!(policy.start(CheckOrigin::Periodic));
    }
    #[test]
    fn hourly_boundary_and_busy_resume_without_burst() {
        let mut policy = UpdateCheckPolicy::default();
        assert!(policy.periodic_due(0, false));
        assert!(!policy.periodic_due(3599, false));
        assert!(!policy.periodic_due(3600, true));
        assert!(policy.periodic_due(3601, false));
        assert!(!policy.periodic_due(3601, false));
        assert!(!policy.periodic_due(7200, false));
        assert!(policy.periodic_due(7201, false));
    }
    #[test]
    fn dismissal_is_version_specific_and_manual_reopens() {
        let mut policy = UpdateCheckPolicy::default();
        policy.dismiss("0.9.0");
        assert!(!policy.should_prompt(CheckOrigin::Startup, "0.9.0"));
        assert!(policy.should_prompt(CheckOrigin::Manual, "0.9.0"));
        assert!(policy.should_prompt(CheckOrigin::Startup, "0.9.1"));
    }
}
