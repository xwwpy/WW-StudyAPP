//! Trusted native file drag bridge shared by the WebView hosts.
use std::{cell::RefCell, path::PathBuf, sync::mpsc::Sender};
use wry::DragDropEvent;

#[derive(Default)]
pub struct DragGate {
    paths: Option<Vec<PathBuf>>,
}

impl DragGate {
    pub fn enter(&mut self, paths: Vec<PathBuf>) -> bool {
        if paths.is_empty() || self.paths.as_ref() == Some(&paths) {
            return false;
        }
        self.paths = Some(paths);
        true
    }

    pub fn leave(&mut self) -> bool {
        self.paths.take().is_some()
    }

    pub fn drop_paths(&mut self, paths: Vec<PathBuf>) -> bool {
        self.paths.take().is_some() && !paths.is_empty()
    }
}

pub fn handler(sender: Sender<String>) -> impl Fn(DragDropEvent) -> bool {
    platform_handler(sender, cfg!(windows))
}

fn platform_handler(sender: Sender<String>, windows: bool) -> impl Fn(DragDropEvent) -> bool {
    let gate = RefCell::new(DragGate::default());
    move |event| {
        // macOS delegates to WebKit's DOM drag handling only when we return
        // false. Keep internal image/text drags (which have no OS paths) there.
        // Wry ignores the return value on Windows; retain its previous result.
        let intercept = windows
            || gate.borrow().paths.is_some()
            || matches!(&event,
                DragDropEvent::Enter { paths, .. } | DragDropEvent::Drop { paths, .. }
                    if !paths.is_empty());
        // Only the OS supplies these paths. Pointer-only Over events and
        // duplicate Enter callbacks must not invalidate the application shell.
        let change = match event {
            DragDropEvent::Enter { paths, .. } => gate
                .borrow_mut()
                .enter(paths.clone())
                .then_some(("enter", paths)),
            DragDropEvent::Drop { paths, .. } => gate
                .borrow_mut()
                .drop_paths(paths.clone())
                .then_some(("drop", paths)),
            DragDropEvent::Leave => gate.borrow_mut().leave().then_some(("leave", Vec::new())),
            _ => None,
        };
        if let Some((phase, paths)) = change {
            let _ = sender.send(
                serde_json::json!({
                    "type": "externalDrag", "phase": phase, "paths": paths
                })
                .to_string(),
            );
        }
        intercept
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    #[test]
    fn native_events_forward_paths_once_and_allow_repeated_drops() {
        let (sender, receiver) = channel();
        let handle = handler(sender);
        let paths = vec![PathBuf::from("/tmp/笔记.md")];
        let position = (4, 8);
        for _ in 0..2 {
            assert!(handle(DragDropEvent::Enter {
                paths: paths.clone(),
                position
            }));
            assert!(handle(DragDropEvent::Enter {
                paths: paths.clone(),
                position
            }));
            assert!(handle(DragDropEvent::Over { position }));
            assert!(handle(DragDropEvent::Drop {
                paths: paths.clone(),
                position
            }));
            assert!(handle(DragDropEvent::Drop {
                paths: paths.clone(),
                position
            }));
            assert_eq!(handle(DragDropEvent::Leave), cfg!(windows));
        }
        let messages: Vec<serde_json::Value> = receiver
            .try_iter()
            .map(|message| serde_json::from_str(&message).unwrap())
            .collect();
        assert_eq!(messages.len(), 4);
        for (message, phase) in messages.iter().zip(["enter", "drop", "enter", "drop"]) {
            assert_eq!(
                message,
                &serde_json::json!({
                    "type": "externalDrag", "phase": phase, "paths": paths
                })
            );
        }
    }

    #[test]
    fn native_cancel_clears_paths_and_internal_dom_drags_emit_nothing() {
        let (sender, receiver) = channel();
        // Exercise WebKit's return-value contract even on a headless Windows runner.
        let handle = platform_handler(sender, false);
        let position = (0, 0);
        assert!(!handle(DragDropEvent::Enter {
            paths: vec![],
            position
        }));
        assert!(!handle(DragDropEvent::Over { position }));
        assert!(!handle(DragDropEvent::Drop {
            paths: vec![],
            position
        }));
        assert!(!handle(DragDropEvent::Leave));
        assert!(receiver.try_recv().is_err());
        let paths = vec![PathBuf::from("/tmp/a.png")];
        assert!(handle(DragDropEvent::Enter {
            paths: paths.clone(),
            position
        }));
        assert!(handle(DragDropEvent::Leave));
        assert!(!handle(DragDropEvent::Leave));
        assert!(handle(DragDropEvent::Drop { paths, position }));
        let messages: Vec<serde_json::Value> = receiver
            .try_iter()
            .map(|message| serde_json::from_str(&message).unwrap())
            .collect();
        assert_eq!(messages.len(), 2);
        assert_eq!(
            messages[1],
            serde_json::json!({
                "type":"externalDrag", "phase":"leave", "paths":[]
            })
        );
    }
}
