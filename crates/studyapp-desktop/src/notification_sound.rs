//! Best-effort, preference-gated semantic sounds; never open a window.
use studyapp_core::{NoticeCategory, NoticeSeverity};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cue {
    Complete,
    Reminder,
    Success,
    Information,
    Warning,
    Error,
}
fn cue(category: NoticeCategory, severity: NoticeSeverity) -> Cue {
    if severity >= NoticeSeverity::Error {
        Cue::Error
    } else if severity == NoticeSeverity::Warning {
        Cue::Warning
    } else if category == NoticeCategory::TimerComplete {
        Cue::Complete
    } else if category == NoticeCategory::ReviewDue {
        Cue::Reminder
    } else if severity == NoticeSeverity::Success {
        Cue::Success
    } else {
        Cue::Information
    }
}

pub fn play(category: NoticeCategory, severity: NoticeSeverity) {
    let sound = cue(category, severity);
    #[cfg(all(windows, not(test)))]
    {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn MessageBeep(kind: u32) -> i32;
        }
        let kind = match sound {
            Cue::Error => 0x10,
            Cue::Warning | Cue::Reminder => 0x30,
            Cue::Success | Cue::Complete => 0x40,
            Cue::Information => 0,
        };
        // The timer's pair is distinct from an ordinary save; no UI-thread sleep.
        std::thread::spawn(move || unsafe {
            let _ = MessageBeep(kind);
            if sound == Cue::Complete {
                std::thread::sleep(std::time::Duration::from_millis(240));
                let _ = MessageBeep(kind);
            }
        });
    }
    #[cfg(all(target_os = "macos", not(test)))]
    {
        let name = match sound {
            Cue::Complete => "Glass",
            Cue::Reminder => "Ping",
            Cue::Success => "Tink",
            Cue::Information => "Pop",
            Cue::Warning => "Sosumi",
            Cue::Error => "Basso",
        };
        std::thread::spawn(move || {
            let _ = std::process::Command::new("/usr/bin/afplay")
                .arg(format!("/System/Library/Sounds/{name}.aiff"))
                .status();
        });
    }
    #[cfg(all(target_os = "linux", not(test)))]
    {
        let name = match sound {
            Cue::Complete => "complete",
            Cue::Reminder => "alarm-clock-elapsed",
            Cue::Success => "message-new-instant",
            Cue::Information => "message",
            Cue::Warning => "dialog-warning",
            Cue::Error => "dialog-error",
        };
        std::thread::spawn(move || {
            use std::process::{Command, Stdio};
            if Command::new("canberra-gtk-play")
                .args(["--id", name])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
            {
                return;
            }
            // Some minimal desktops omit libcanberra's CLI. Use their existing
            // sound theme/player if present; never install or download audio.
            let path = std::path::PathBuf::from(format!(
                "/usr/share/sounds/freedesktop/stereo/{name}.oga"
            ));
            if path.is_file() {
                for player in ["paplay", "pw-play"] {
                    if Command::new(player)
                        .arg(&path)
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status()
                        .is_ok_and(|status| status.success())
                    {
                        return;
                    }
                }
            }
        });
    }
    #[cfg(test)]
    let _ = sound;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failure_severity_overrides_category_and_completions_remain_distinct() {
        assert_eq!(
            cue(NoticeCategory::TimerComplete, NoticeSeverity::Error),
            Cue::Error
        );
        assert_eq!(
            cue(NoticeCategory::SaveSuccess, NoticeSeverity::Warning),
            Cue::Warning
        );
        assert_eq!(
            cue(NoticeCategory::TimerComplete, NoticeSeverity::Information),
            Cue::Complete
        );
        assert_eq!(
            cue(NoticeCategory::ReviewDue, NoticeSeverity::Information),
            Cue::Reminder
        );
        assert_ne!(
            cue(NoticeCategory::SaveSuccess, NoticeSeverity::Success),
            cue(NoticeCategory::OperationResult, NoticeSeverity::Information)
        );
    }
}
