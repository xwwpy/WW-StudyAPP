//! Native runner-only pin acknowledgement, using GPUI-dispatched button clicks
//! and independent, passive OS observations (not physical pointer coverage).
use crate::{
    app::{Page, StudyApp},
    smoke_library::until,
    timer_widget_window,
};
use gpui_kit::{test::TestWindowExt, *};
use serde_json::{Value, json};
use std::{
    path::Path,
    time::{Duration, Instant},
};
use studyapp_core::Command;

fn observe(window: AnyWindowHandle, cx: &mut AsyncApp) -> Value {
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let native = timer_widget_window::pin_observation(window).unwrap();
            json!({"native": native, "checked": window.find("widget-pin").checked(),
            "label": window.find("widget-pin").label()})
        })
        .unwrap()
}

async fn acknowledged(widget: AnyWindowHandle, target: bool, cx: &mut AsyncApp) -> Value {
    let started = Instant::now();
    loop {
        let state = observe(widget, cx);
        let label = if target { "取消置顶" } else { "置顶" };
        if state["native"]["pinned"] == target
            && state["checked"] == target
            && state["label"] == label
        {
            widget
                .update(cx, |_, window, _| {
                    // Only probe an acknowledged, settled request: polling an
                    // in-flight EWMH transition would make two reads race the WM.
                    let before = timer_widget_window::pin_observation(window).unwrap();
                    assert!(!timer_widget_window::decorate(window, !target, false).unwrap());
                    assert_eq!(
                        timer_widget_window::pin_observation(window).unwrap(),
                        before,
                        "opposite-level observation must never mutate the native window"
                    );
                })
                .unwrap();
            return state;
        }
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "pin acknowledgement: {state}"
        );
        cx.background_executor()
            .timer(Duration::from_millis(40))
            .await;
    }
}

pub async fn exercise(
    main: AnyWindowHandle,
    app: &Entity<StudyApp>,
    database: &Path,
    cx: &mut AsyncApp,
) {
    assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    let original = app.read_with(cx, |app, _| {
        app.snapshot.as_ref().unwrap().settings.timer_widget.pinned
    });
    main.update(cx, |_, window, cx| {
        app.update(cx, |app, cx| {
            app.navigate(Page::Today, window, cx);
            assert_eq!(app.page, Page::Today);
            assert!(app.timer_widget.is_none());
        });
        // Complete the departure from the preceding native WebView before
        // starting the worker acknowledgement deadline. Its hide/layout work
        // must not be charged to a command that has not been sent yet.
        window.render_frame(cx);
        app.update(cx, |app, cx| {
            app.send(Command::SetTimerWidgetPinned(false), cx);
        });
    })
    .unwrap();
    let mut last = None;
    until(
        main,
        app,
        "prepare unpinned widget",
        |app, _| {
            let state = (
                app.snapshot.as_ref().unwrap().settings.timer_widget.pinned,
                app.preferences_ui
                    .saved
                    .as_ref()
                    .map(|p| p.settings.timer_widget.pinned),
                app.storage_busy,
            );
            if last != Some(state) {
                eprintln!("native pin preparation: snapshot/saved/busy={state:?}");
                last = Some(state);
            }
            !state.0 && !state.2
        },
        cx,
    )
    .await;
    let document = app.read_with(cx, |app, cx| {
        let editor = app.note_content.read(cx);
        (
            editor.value().to_owned(),
            editor.generation(),
            editor.revision(),
        )
    });
    main.update(cx, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("timer-show-desktop").checked(), Some(false));
        window.click("timer-show-desktop", cx);
    })
    .unwrap();
    until(
        main,
        app,
        "open native pin fixture",
        |app, _| app.timer_widget.is_some(),
        cx,
    )
    .await;
    let widget = app.read_with(cx, |app, _| app.timer_widget.unwrap());
    // Let the existing content fit settle before comparing native frames.
    cx.background_executor()
        .timer(Duration::from_millis(500))
        .await;
    let mut states = vec![acknowledged(widget, false, cx).await];
    for target in [true, false] {
        widget
            .update(cx, |_, window, cx| window.click("widget-pin", cx))
            .unwrap();
        until(
            main,
            app,
            "persist requested widget pin",
            |app, _| app.snapshot.as_ref().unwrap().settings.timer_widget.pinned == target,
            cx,
        )
        .await;
        let state = acknowledged(widget, target, cx).await;
        assert_eq!(
            state["native"]["bounds"], states[0]["native"]["bounds"],
            "pin must not move/resize the widget"
        );
        assert_eq!(
            state["native"]["focus"], states[0]["native"]["focus"],
            "pin must not steal native focus"
        );
        assert_eq!(state["native"]["managed"], true);
        states.push(state);
        widget
            .update(cx, |_, window, _| {
                crate::smoke::capture(
                    window,
                    database.with_file_name(format!("native-timer-pin-{target}.png")),
                )
                .unwrap();
            })
            .unwrap();
    }
    main.update(cx, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("timer-show-desktop").checked(), Some(true));
        window.click("timer-show-desktop", cx);
        app.update(cx, |app, cx| {
            assert!(app.timer_widget.is_none());
            let editor = app.note_content.read(cx);
            assert_eq!(
                (
                    editor.value().to_owned(),
                    editor.generation(),
                    editor.revision()
                ),
                document
            );
            app.send(Command::SetTimerWidgetPinned(original), cx);
        });
    })
    .unwrap();
    until(
        main,
        app,
        "restore widget pin preference",
        |app, _| app.snapshot.as_ref().unwrap().settings.timer_widget.pinned == original,
        cx,
    )
    .await;
    let closed = app.read_with(cx, |app, _| app.timer_widget.is_none());
    std::fs::write(
        database.with_file_name("native-timer-pin.json"),
        serde_json::to_vec_pretty(&json!({
            "schema": 1, "states": states, "desktopClosed": closed,
        }))
        .unwrap(),
    )
    .unwrap();
}
