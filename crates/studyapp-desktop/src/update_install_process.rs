//! Parse one process-state/identity observation without treating query errors as exit.
use sha2::{Digest, Sha256};

pub struct Snapshot {
    pub identity: u64,
    pub exited: bool,
}
impl Snapshot {
    pub fn matches_live(&self, expected: u64) -> bool {
        !self.exited && self.identity == expected
    }
}
pub fn parse(code: Option<i32>, stdout: &str, stderr: &str) -> Result<Option<Snapshot>, String> {
    if code == Some(1) && stdout.trim().is_empty() && stderr.trim().is_empty() {
        return Ok(None);
    }
    if code != Some(0) || !stderr.trim().is_empty() {
        return Err("Cannot query process state".into());
    }
    let row = stdout.trim();
    let (state, started) = row
        .split_once(char::is_whitespace)
        .ok_or("Invalid process snapshot")?;
    let started = started.trim();
    if row.lines().count() != 1
        || !matches!(
            state.as_bytes().first(),
            Some(b'R' | b'S' | b'I' | b'T' | b'U' | b'Z')
        )
        || chrono::NaiveDateTime::parse_from_str(started, "%a %b %e %T %Y").is_err()
    {
        return Err("Invalid process snapshot".into());
    }
    let hash = Sha256::digest(started.as_bytes());
    Ok(Some(Snapshot {
        identity: u64::from_le_bytes(
            hash[..8]
                .try_into()
                .map_err(|_| "Invalid process identity")?,
        ),
        exited: state.starts_with('Z'),
    }))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn update_install_process_live_and_zombie_share_identity_but_not_liveness() {
        let live = parse(Some(0), "S+ Fri Oct 30 02:00:00 2026\n", "")
            .unwrap()
            .unwrap();
        let zombie = parse(Some(0), "Z Fri Oct 30 02:00:00 2026\n", "")
            .unwrap()
            .unwrap();
        assert_eq!(live.identity, zombie.identity);
        assert!(live.matches_live(live.identity));
        assert!(!zombie.matches_live(live.identity));
        assert!(!live.matches_live(live.identity.wrapping_add(1)));
    }
    #[test]
    fn update_install_process_absence_is_distinct_from_query_failure() {
        assert!(parse(Some(1), "", "").unwrap().is_none());
        for (code, out, err) in [
            (Some(0), "", ""),
            (Some(1), "", "denied"),
            (None, "", ""),
            (Some(0), "? Fri Oct 30 02:00:00 2026", ""),
            (Some(0), "R", ""),
            (Some(0), "R date\nS date", ""),
        ] {
            assert!(parse(code, out, err).is_err());
        }
    }
}
