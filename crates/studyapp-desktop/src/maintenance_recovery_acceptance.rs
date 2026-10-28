//! Actual snapshot recovery UI. No synthetic executor or restoration shortcut.
use super::*;
use gpui_kit::test::TestWindowExt;
use std::{cell::Cell, io::Write, rc::Rc, time::Duration};

pub(crate) fn run_recovery_acceptance(request: RecoveryRequest, evidence: PathBuf) -> Result<()> {
    ensure!(
        capture_allowed(
            evidence
                .parent()
                .context("Recovery evidence parent absent")?
        ) && std::env::var("RUNNER_OS").as_deref() == Ok("Windows")
            && !evidence.try_exists()?,
        "Recovery UI acceptance requires fresh Windows runner evidence"
    );
    let snapshot = crate::maintenance_execute::recovery_snapshot(&request)?;
    ensure!(
        same_absolute_path(&std::env::current_exe()?, &snapshot.join("Uninstall.exe"))?,
        "Actual recovery must launch the verified standalone snapshot entry"
    );
    let transaction = request.prepared.transaction_id.clone();
    let install = request.prepared.install_root.clone();
    let screenshot = evidence.parent().unwrap().join("recovery-ui.png");
    ensure!(
        !screenshot.try_exists()?,
        "Recovery UI evidence already exists"
    );
    let calls = Rc::new(Cell::new(0usize));
    let recorded = calls.clone();
    run_with_opened(
        Mode::Recover(Some(Box::new(request))),
        None,
        Some(Box::new(move |handle, view, cx| {
            view.update(cx, |view, _| {
            view.handoff_observer = Some(Box::new(move |handoff| {
                recorded.set(recorded.get() + 1);
                let bytes = serde_json::to_vec_pretty(&serde_json::json!({
                    "stage":handoff.stage,"helper_pid":handoff.helper_pid,"helper_created":handoff.helper_created,
                    "transaction_id":transaction,"install_root":install,"snapshot":snapshot,
                    "actual_recovery_button":true,"production_ready_observed":true
                }))?;
                let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&evidence)?;
                file.write_all(&bytes)?;file.sync_all()?;
                Ok(())
            }));
        });
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
                handle
                    .update(cx, |_, window, cx| {
                        window.render_frame(cx);
                        window.find("maintenance-recover");
                        window.render_to_image().unwrap().save(&screenshot).unwrap();
                        // This is the actual production listener. Its successful live
                        // IPC READY triggers our observer, then the real UI exits.
                        window.click("maintenance-recover", cx);
                    })
                    .unwrap();
            })
            .detach();
        })),
    )?;
    ensure!(
        calls.get() == 1,
        "Actual recovery button did not authenticate exactly one helper"
    );
    Ok(())
}
