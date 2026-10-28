//! CI-only native performance evidence using in-memory storage reports.
use crate::{
    app::{Page, StudyApp},
    settings_ui::{SettingsSection, StoragePage},
    smoke_library::scroll_settings,
    storage_backend::{CopyState, ManagedCopy, StorageReport},
};
use gpui_kit::{test::TestWindowExt, *};
use std::{path::Path, time::Instant};

fn timing(mut samples: Vec<u64>) -> serde_json::Value {
    if samples.is_empty() {
        return serde_json::Value::Null;
    }
    samples.sort_unstable();
    let count = samples.len();
    serde_json::json!({
        "count": count,
        "p50_us": samples[count.div_ceil(2) - 1],
        "p95_us": samples[(count * 95).div_ceil(100) - 1],
        "samples_us_sorted": samples,
    })
}

fn fixture(data_path: &str, seed: &ManagedCopy, count: usize) -> StorageReport {
    let mut report = StorageReport {
        data_path: data_path.to_owned(),
        database_bytes: 1024,
        managed_bytes: seed.bytes.saturating_mul(count as u64),
        scanned_at: chrono::Utc::now().timestamp(),
        copies: (0..count)
            .map(|ix| ManagedCopy {
                note_id: format!("storage-bench-{count}-{ix}"),
                title: format!("空间管理性能样本 {ix:04} — {}", seed.title),
                path: seed.path.clone(),
                bytes: seed.bytes,
                state: CopyState::Present,
                // The report is a rendering fixture. It never grants file maintenance.
                deletable: false,
                reason: None,
            })
            .collect(),
        ..Default::default()
    };
    report.prepare_presentation();
    report
}

/// Run only in the GitHub native-smoke job. No worker command or file mutation
/// is issued; the original report, scroll state and settings section are restored.
pub fn measure(window: &mut Window, view: &Entity<StudyApp>, database: &Path, cx: &mut App) {
    let (
        saved_report,
        saved_ui,
        saved_page,
        saved_section,
        saved_transition,
        saved_scroll,
        saved_storage_page,
    ) = view.update(cx, |app, cx| {
        assert!(!app.storage_busy, "storage fixture requires an idle worker");
        let state = (
            app.storage_report.take(),
            std::mem::take(&mut app.storage_ui),
            app.page,
            app.preferences_ui.section,
            app.preferences_ui.section_changed_at,
            std::mem::replace(&mut app.settings_scroll, ScrollHandle::new()),
            app.preferences_ui.storage_page,
        );
        app.page = Page::Settings;
        app.select_settings_section(SettingsSection::Storage, cx);
        app.select_storage_page(StoragePage::Copies, cx);
        app.preferences_ui.section_changed_at = None;
        state
    });
    let seed = saved_report
        .as_ref()
        .and_then(|report| report.copies.first())
        .cloned()
        .unwrap_or_else(|| ManagedCopy {
            note_id: "storage-bench-seed".into(),
            title: "只读显示样本".into(),
            path: database
                .with_file_name("render-fixture.md")
                .to_string_lossy()
                .into_owned(),
            bytes: 2048,
            state: CopyState::Present,
            deletable: false,
            reason: None,
        });
    let data_path = view.read(cx).last_library.clone();
    view.update(cx, |app, cx| {
        app.storage_report = Some(fixture(&data_path, &seed, 100));
        cx.notify();
    });
    window.render_frame(cx);
    // The storage report is the final card. Position it once using the bounded
    // virtual fixture, so the 1000-row eager control needs only one draw.
    scroll_settings(window, -100_000., cx);
    window.render_frame(cx);
    let bounds = window.find("storage-copy-list").bounds();
    let clip = window
        .find("settings-section-scroll")
        .bounds()
        .intersect(&bounds);
    assert!(
        clip.size.height > px(200.),
        "storage performance viewport is clipped"
    );

    let mut results = Vec::new();
    for (count, eager, samples) in [
        (100, false, 20),
        (100, true, 3),
        (1000, false, 20),
        (1000, true, 1),
    ] {
        eprintln!("storage timing: {count} copies, eager={eager}");
        let prepare_start = Instant::now();
        let report = fixture(&data_path, &seed, count);
        let prepare_us = prepare_start.elapsed().as_micros() as u64;
        let descriptor_count = report.presentation.copies.len();
        view.update(cx, |app, cx| {
            app.storage_report = Some(report);
            app.storage_ui = Default::default();
            app.storage_ui.eager_baseline = eager;
            cx.notify();
        });
        let mut frames = Vec::new();
        let mut builds = Vec::new();
        for _ in 0..samples {
            view.update(cx, |_, cx| cx.notify());
            let before = view.read(cx).storage_render_work().0;
            let start = Instant::now();
            window.render_frame(cx);
            frames.push(start.elapsed().as_micros() as u64);
            let (after, rows) = view.read(cx).storage_render_work();
            builds.push(after - before);
            if eager {
                assert_eq!(
                    rows as usize, descriptor_count,
                    "eager control must use every real row builder"
                );
            } else {
                assert!(
                    rows > 0 && rows < 32,
                    "storage built {rows} rows for {count} copies"
                );
            }
        }
        let first_frame_us = frames[0];
        let mut wheels = Vec::new();
        let mut wheel_builds = Vec::new();
        // A single 1000-row eager frame is enough to prove the scale difference.
        // Avoid making the native CI job repeatedly lay out that entire control.
        if count == 100 || !eager {
            let wheel_samples = if eager { 2 } else { 10 };
            for ix in 0..wheel_samples {
                let before = view.read(cx).storage_render_work().0;
                let start = Instant::now();
                window.scroll(
                    "storage-copy-list",
                    ScrollDelta::Pixels(point(
                        px(0.),
                        px(if ix < wheel_samples / 2 { -100. } else { 100. }),
                    )),
                    cx,
                );
                window.render_frame(cx);
                wheels.push(start.elapsed().as_micros() as u64);
                wheel_builds.push(view.read(cx).storage_render_work().0 - before);
            }
        }
        if !eager {
            view.update(cx, |app, cx| {
                app.storage_ui.scroll.scroll_to_end();
                cx.notify();
            });
            window.render_frame(cx);
            let last_id =
                SharedString::from(format!("copy-history-storage-bench-{count}-{}", count - 1));
            assert!(
                window.try_find(last_id).is_some(),
                "last managed-copy row is unreachable"
            );
            assert!(
                view.read(cx).storage_render_work().1 < 32,
                "bottom viewport constructed all copies"
            );
            let list = &view.read(cx).storage_ui.scroll;
            assert!(
                (f32::from(
                    list.scroll_px_offset_for_scrollbar().y + list.max_offset_for_scrollbar().y
                ))
                .abs()
                    < 1.
            );
        }
        results.push(serde_json::json!({
            "copies": count,
            "mode": if eager { "eager_control" } else { "virtual" },
            "report_preparation_us": prepare_us,
            "first_frame_us": first_frame_us,
            "render_frame": timing(frames),
            "row_builder_calls_per_frame": builds,
            "wheel_dispatch_and_render": timing(wheels),
            "row_builder_calls_per_wheel_frame": wheel_builds,
        }));
    }
    view.update(cx, |app, cx| {
        app.storage_report = saved_report;
        app.storage_ui = saved_ui;
        app.select_settings_section(saved_section, cx);
        app.select_storage_page(saved_storage_page, cx);
        app.preferences_ui.section_changed_at = saved_transition;
        app.settings_scroll = saved_scroll;
        app.page = saved_page;
        cx.notify();
    });
    window.render_frame(cx);
    std::fs::write(database.with_file_name("storage-render-timing.json"), serde_json::to_string_pretty(&serde_json::json!({
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "measurement": "Same-process GPUI row builders; refresh/draw/clear timing excludes OS input transit and compositor presentation. In-memory report fixtures; no note files are created or deleted. Eager control is compiled only for ui-test and is not a historical executable comparison.",
        "viewport": { "width": f32::from(window.viewport_size().width), "height": f32::from(window.viewport_size().height) },
        "results": results,
    })).unwrap()).unwrap();
}
