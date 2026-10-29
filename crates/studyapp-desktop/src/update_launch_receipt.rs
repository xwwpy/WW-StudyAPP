use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchReceipt {
    pub nonce: String,
    pub version: String,
    pub sha256: String,
    pub pid: u32,
}
impl LaunchReceipt {
    pub fn matches(&self, nonce: &str, version: &str, sha256: &str) -> bool {
        self.pid > 1 && self.nonce == nonce && self.version == version && self.sha256 == sha256
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn update_launch_receipt_binds_actual_candidate_and_fresh_attempt() {
        let receipt = LaunchReceipt {
            nonce: "fresh nonce".into(),
            version: "0.1.4".into(),
            sha256: "exact candidate digest".into(),
            pid: 42,
        };
        assert!(receipt.matches("fresh nonce", "0.1.4", "exact candidate digest"));
        assert!(!receipt.matches("previous nonce", "0.1.4", "exact candidate digest"));
        assert!(!receipt.matches("fresh nonce", "0.1.3", "exact candidate digest"));
        assert!(!receipt.matches("fresh nonce", "0.1.4", "replaced candidate"));
        let invalid = LaunchReceipt { pid: 0, ..receipt };
        assert!(!invalid.matches("fresh nonce", "0.1.4", "exact candidate digest"));
    }
}

pub fn confirm_stopped(
    observation: Result<bool, String>,
    terminate: impl FnOnce() -> Result<(), String>,
    wait: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    if observation? {
        terminate()?;
        wait()?;
    }
    Ok(())
}
#[cfg(test)]
mod recovery_tests {
    use super::*;
    #[test]
    fn update_recovery_never_treats_unknown_process_state_as_confirmed_exit() {
        for error in [
            "kqueue failed",
            "identity query failed",
            "path query failed",
        ] {
            assert!(
                confirm_stopped(
                    Err(error.into()),
                    || panic!("unknown identity must not be killed"),
                    || panic!("not terminated")
                )
                .is_err()
            );
        }
        assert!(confirm_stopped(Ok(true), || Ok(()), || Err("exit not observed".into())).is_err());
        assert!(
            confirm_stopped(
                Ok(false),
                || panic!("already exited"),
                || panic!("already exited")
            )
            .is_ok()
        );
    }
}
