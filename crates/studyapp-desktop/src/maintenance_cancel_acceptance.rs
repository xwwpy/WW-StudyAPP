//! Actual cancellation events with the production executor bound, runner only.
use super::*;
use gpui_kit::test::TestWindowExt;
use std::{cell::Cell, path::PathBuf, rc::Rc, time::Duration};

pub(crate) fn run_cancel_acceptance(
    discovery: InstallDiscovery,
    context: DiscoveryContext,
    stage: ConfirmationStage,
    screenshot: PathBuf,
) -> Result<usize> {
    let runner = PathBuf::from(std::env::var_os("RUNNER_TEMP").context("RUNNER_TEMP required")?);
    ensure!(
        std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true")
            && std::env::var("RUNNER_OS").as_deref() == Ok("Windows")
            && absolute_components(&screenshot)?.starts_with(&absolute_components(&runner)?)
            && !screenshot.try_exists()?,
        "Cancellation views require fresh Windows runner evidence"
    );
    ensure!(
        matches!(
            stage,
            ConfirmationStage::Selection
                | ConfirmationStage::DatabaseReview
                | ConfirmationStage::DatabaseFinal
        ),
        "Invalid cancel stage"
    );
    let receipt = discovery
        .receipt
        .clone()
        .context("Verified fixture receipt required")?;
    let calls = Rc::new(Cell::new(0usize));
    let executed = calls.clone();
    let closed = Rc::new(Cell::new(false));
    let observed_closed = closed.clone();
    run_window(
        discovery,
        Box::new(move |confirmed| {
            executed.set(executed.get() + 1);
            // Identical executor boundary to run_interactive; a mistaken click must
            // enter production verification/handoff, never an accepting fake.
            let intent =
                crate::maintenance_execute::prepare_uninstall(confirmed, &receipt, &context)?;
            crate::maintenance_handoff::begin_uninstall_handoff(intent)?;
            Ok("已交接至独立卸载程序。".into())
        }),
        Some(Box::new(move |handle, view, cx| {
            cx.on_window_closed(move |cx, _| {
                if cx.windows().is_empty() {
                    observed_closed.set(true);
                }
            })
            .detach();
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
                handle
                    .update(cx, |_, window, cx| {
                        window.render_frame(cx);
                        window.click("maintenance-config", cx);
                        window.render_frame(cx);
                        window.click("maintenance-database", cx);
                        window.render_frame(cx);
                        if stage != ConfirmationStage::Selection {
                            window.click("maintenance-next", cx);
                            window.render_frame(cx);
                        }
                        if stage == ConfirmationStage::DatabaseFinal {
                            window.click("maintenance-next", cx);
                            window.render_frame(cx);
                        }
                        assert_eq!(view.read(cx).controller.stage(), stage);
                        assert_eq!(
                            view.read(cx).controller.selection,
                            DataSelection {
                                configuration: true,
                                database: true
                            }
                        );
                        window.render_to_image().unwrap().save(&screenshot).unwrap();
                        window.click("maintenance-cancel", cx);
                    })
                    .unwrap();
            })
            .detach();
        })),
    )?;
    ensure!(
        closed.get() && calls.get() == 0,
        "Actual cancellation invoked the executor or failed to close"
    );
    Ok(calls.get())
}
