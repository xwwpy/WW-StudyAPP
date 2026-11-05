use super::{UpdateEvent, UpdateReply};
use crate::update_backend::{UpdateErrorKind, failure};
use gpui_kit::TestAppContext;
use std::{cell::RefCell, rc::Rc};
use studyapp_core::NoticeCategory;
use studyapp_desktop::update_policy::CheckOrigin;

#[gpui_kit::test]
fn update_outcomes_are_classified_without_exposing_raw_install_errors(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::apply(&Default::default(), false, cx);
    });
    let (panel, visual) = cx.add_window_view(|_, _| super::tests::panel());
    let events = Rc::new(RefCell::new(Vec::new()));
    let output = events.clone();
    let _subscription = visual.update(|_, cx| {
        cx.subscribe(&panel, move |_, event: &UpdateEvent, _| {
            if let UpdateEvent::Notification(event) = event {
                output.borrow_mut().push(event.clone());
            }
        })
    });
    let manifest = crate::update_manifest::UpdateManifest {
        version: semver::Version::parse("0.9.0").unwrap(),
        notes: "新功能".into(),
        history: Vec::new(),
        pub_date: None,
        setup_url: "https://github.com/xwwpy/WW-StudyAPP/releases/download/v0.9.0/setup.exe".into(),
        signature: "fixture".into(),
        signed_envelope: String::new(),
        size: 10,
        sha256: "a".repeat(64),
    };
    panel.update(visual, |panel, cx| {
        panel
            .sender
            .send(UpdateReply::NsisChecked(
                CheckOrigin::Startup,
                Err(failure(UpdateErrorKind::Network)),
            ))
            .unwrap();
        panel
            .sender
            .send(UpdateReply::NsisChecked(
                CheckOrigin::Manual,
                Err(failure(UpdateErrorKind::Network)),
            ))
            .unwrap();
        panel
            .sender
            .send(UpdateReply::NsisDownloaded(Box::new(Err(
                "private-install-location secret-stack".into(),
            ))))
            .unwrap();
        panel
            .sender
            .send(UpdateReply::Prepared(
                String::new(),
                Box::new(Err(failure(UpdateErrorKind::Verification))),
            ))
            .unwrap();
        for _ in 0..2 {
            panel
                .sender
                .send(UpdateReply::NsisChecked(
                    CheckOrigin::Periodic,
                    Ok(Some(manifest.clone())),
                ))
                .unwrap();
        }
        panel.poll(cx);
        panel.show_dialog(cx);
        panel.install_failed("private-install-location secret-stack".into(), cx);
        let content = panel.dialog_content().unwrap();
        let studyapp_ui::update_dialog::UpdateDialogPhase::Failed(message) = content.phase else {
            panic!("missing failure phase")
        };
        assert!(!message.contains("private-install-location"));
        assert!(!message.contains("secret-stack"));
    });
    visual.run_until_parked();
    let events = events.borrow();
    assert_eq!(
        events.iter().map(|e| e.category).collect::<Vec<_>>(),
        vec![
            NoticeCategory::UpdateCheckFailure,
            NoticeCategory::UpdateDownloadFailure,
            NoticeCategory::UpdateInstallFailure,
            NoticeCategory::UpdateAvailable,
            NoticeCategory::UpdateInstallFailure
        ]
    );
    for event in events.iter() {
        event.validate().unwrap();
        assert!(!event.summary.contains("private-install-location"));
        assert!(!event.summary.contains("secret-stack"));
        assert!(event.action.is_some());
    }
    assert!(!panel.read_with(visual, |panel, _| panel.busy));
}
