/// A manual request already in flight also satisfies the single startup attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckOrigin {
    Startup,
    Manual,
}

#[derive(Default)]
pub struct UpdateCheckPolicy {
    startup_claimed: bool,
    in_flight: bool,
}

impl UpdateCheckPolicy {
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
