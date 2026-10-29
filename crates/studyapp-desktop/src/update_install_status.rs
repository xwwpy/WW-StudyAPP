//! Complete transaction records and recovery notification ordering.
use std::{fs, path::Path};
type Result<T> = std::result::Result<T, String>;

pub(super) fn publish(
    dir: &Path,
    state: &str,
    message: &str,
    write: impl FnOnce(&Path, &[u8]) -> Result<()>,
) -> Result<()> {
    let bytes = serde_json::to_vec(&serde_json::json!({"state":state,"message":message}))
        .map_err(|e| e.to_string())?;
    let id = uuid::Uuid::new_v4();
    let temporary = dir.join(format!("record-{id}.tmp"));
    write(&temporary, &bytes)?;
    fs::rename(temporary, dir.join(format!("status-{state}-{id}.json"))).map_err(|e| e.to_string())
}
pub(super) fn recover<T>(
    dir: &Path,
    message: &str,
    write: impl FnOnce(&Path, &[u8]) -> Result<()>,
    restart: impl FnOnce() -> Result<T>,
) -> Result<T> {
    publish(dir, "failed", message, write)?;
    restart()
}
pub(super) fn failure(
    dir: &Path,
    read: impl Fn(&Path) -> Result<Vec<u8>>,
) -> Result<Option<String>> {
    let mut records = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_name().to_string_lossy().starts_with("status-") {
            records.push((
                entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .map_err(|e| e.to_string())?,
                entry.path(),
            ));
        }
    }
    // Filesystem timestamp resolution can tie adjacent records. In a transaction
    // that has failed, a tied progress/recovered record must not hide the failure.
    records.sort_by_key(|v| {
        (
            v.0,
            v.1.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("status-failed-")),
        )
    });
    if let Some((_, path)) = records.last() {
        let value: serde_json::Value =
            serde_json::from_slice(&read(path)?).map_err(|e| e.to_string())?;
        if value["state"] == "failed" {
            return Ok(Some(format!(
                "上次更新未完成：{}。记录：{}",
                value["message"].as_str().unwrap_or("未知错误"),
                dir.display()
            )));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, path::PathBuf};
    fn directory() -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("studyapp-status-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        path
    }
    fn write(path: &Path, bytes: &[u8]) -> Result<()> {
        fs::write(path, bytes).map_err(|e| e.to_string())
    }
    fn read(path: &Path) -> Result<Vec<u8>> {
        fs::read(path).map_err(|e| e.to_string())
    }
    #[test]
    fn update_install_status_recovery_callback_already_reads_failure() {
        for message in [
            "authorization denied",
            "NSIS failed",
            "bundle installation failed",
            "previous app restored",
        ] {
            let dir = directory();
            recover(&dir, message, write, || {
                assert!(failure(&dir, read)?.is_some_and(|value| value.contains(message)));
                Ok(())
            })
            .unwrap();
        }
    }
    #[test]
    fn update_install_status_partial_record_never_hides_existing_failure() {
        let dir = directory();
        publish(&dir, "failed", "old failure", write).unwrap();
        publish(&dir, "failed", "new failure", |path, bytes| {
            let mut file = fs::File::create(path).unwrap();
            file.write_all(&bytes[..5]).unwrap();
            assert!(failure(&dir, read)?.unwrap().contains("old failure"));
            file.write_all(&bytes[5..]).unwrap();
            Ok(())
        })
        .unwrap();
    }
    #[test]
    fn update_install_status_failed_wins_equal_timestamp() {
        let dir = directory();
        let failed = dir.join("status-failed-first.json");
        let recovered = dir.join("status-recovered-last.json");
        write(&failed, br#"{"state":"failed","message":"restored"}"#).unwrap();
        write(&recovered, br#"{"state":"recovered","message":"restored"}"#).unwrap();
        let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1000);
        for path in [&failed, &recovered] {
            fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(time))
                .unwrap();
        }
        assert!(failure(&dir, read).unwrap().is_some());
    }
}
