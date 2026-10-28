//! A native folder dialog whose reply is scoped to the originating draft.

use std::{path::PathBuf, sync::mpsc};

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct FolderRequest {
    library: String,
    epoch: u64,
    sequence: u64,
}
struct FolderReply {
    request: FolderRequest,
    path: Option<PathBuf>,
}

pub struct FolderPicker {
    epoch: u64,
    sequence: u64,
    active: Option<FolderRequest>,
    sender: mpsc::Sender<FolderReply>,
    receiver: mpsc::Receiver<FolderReply>,
}
impl Default for FolderPicker {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            epoch: 0,
            sequence: 0,
            active: None,
            sender,
            receiver,
        }
    }
}
fn initial_directory(path: Option<PathBuf>) -> Option<PathBuf> {
    let path = path?;
    path.ancestors()
        .find(|path| path.is_absolute() && path.is_dir())
        .map(PathBuf::from)
}
/// Blocking native chooser. Call on a background executor/thread, including
/// initial directory probing because network/NAS metadata may block.
pub fn pick_folder(title: &str, current: Option<PathBuf>) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new().set_title(title);
    if let Some(initial) = initial_directory(current) {
        dialog = dialog.set_directory(initial);
    }
    dialog.pick_folder()
}
impl FolderPicker {
    pub fn invalidate(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.active = None;
    }
    pub(crate) fn begin(&mut self, library: &str) -> FolderRequest {
        self.sequence = self.sequence.wrapping_add(1);
        let request = FolderRequest {
            library: library.into(),
            epoch: self.epoch,
            sequence: self.sequence,
        };
        self.active = Some(request.clone());
        request
    }
    #[cfg(all(test, feature = "ui-test"))]
    pub(crate) fn reply_test(&self, request: FolderRequest, path: &str) {
        self.sender
            .send(FolderReply {
                request,
                path: Some(path.into()),
            })
            .unwrap();
    }
    pub fn choose(&mut self, library: &str, current: Option<PathBuf>, title: &str) {
        let request = self.begin(library);
        let sender = self.sender.clone();
        let title = title.to_owned();
        std::thread::spawn(move || {
            let path = pick_folder(&title, current);
            let _ = sender.send(FolderReply { request, path });
        });
    }
    pub fn poll(&mut self, library: &str) -> Option<PathBuf> {
        let mut selected = None;
        while let Ok(reply) = self.receiver.try_recv() {
            if self.active.as_ref() != Some(&reply.request) {
                continue;
            }
            self.active = None;
            if reply.request.library == library && reply.request.epoch == self.epoch {
                selected = reply.path;
            }
        }
        selected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_picker_does_not_modify_a_field_and_consumes_the_request() {
        let mut picker = FolderPicker::default();
        let request = picker.begin("library-a");
        picker
            .sender
            .send(FolderReply {
                request: request.clone(),
                path: None,
            })
            .unwrap();
        assert!(picker.poll("library-a").is_none());
        picker
            .sender
            .send(FolderReply {
                request,
                path: Some("late duplicate".into()),
            })
            .unwrap();
        assert!(picker.poll("library-a").is_none());
    }

    #[test]
    fn only_latest_request_from_current_library_and_epoch_is_accepted() {
        let mut picker = FolderPicker::default();
        let stale = picker.begin("library-a");
        let current = picker.begin("library-a");
        picker
            .sender
            .send(FolderReply {
                request: stale,
                path: Some("stale".into()),
            })
            .unwrap();
        picker
            .sender
            .send(FolderReply {
                request: current.clone(),
                path: Some("chosen".into()),
            })
            .unwrap();
        assert_eq!(picker.poll("library-a"), Some("chosen".into()));
        picker
            .sender
            .send(FolderReply {
                request: current,
                path: Some("duplicate".into()),
            })
            .unwrap();
        assert!(picker.poll("library-a").is_none());
        let before_reset = picker.begin("library-a");
        picker.invalidate();
        picker
            .sender
            .send(FolderReply {
                request: before_reset,
                path: Some("reset-stale".into()),
            })
            .unwrap();
        assert!(picker.poll("library-a").is_none());
        let before_switch = picker.begin("library-a");
        picker
            .sender
            .send(FolderReply {
                request: before_switch,
                path: Some("wrong-library".into()),
            })
            .unwrap();
        assert!(picker.poll("library-b").is_none());
    }

    #[test]
    fn initial_directory_uses_current_folder_or_nearest_existing_parent() {
        let root = tempfile::tempdir().unwrap().keep();
        assert_eq!(initial_directory(Some(root.clone())), Some(root.clone()));
        assert_eq!(
            initial_directory(Some(root.join("missing").join("child"))),
            Some(root)
        );
        assert_eq!(initial_directory(None), None);
    }
}
