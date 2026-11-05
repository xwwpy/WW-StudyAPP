use studyapp_core::{Command, Engine, Settings, TimerKind, TimerState, TimerWidgetSettings};

fn fixture() -> (std::path::PathBuf, Engine) {
    let dir = tempfile::tempdir().unwrap().keep();
    let engine = Engine::open(&dir.join("timers.sqlite"), 1000).unwrap();
    (dir, engine)
}
fn start(engine: &mut Engine, kind: TimerKind, target: Option<u64>) {
    engine
        .apply(
            Command::StartTimer {
                kind,
                target_seconds: target,
                label: "阅读计划".into(),
            },
            1000,
        )
        .unwrap();
}

#[test]
fn legacy_timer_keeps_its_progress_and_defaults_to_study() {
    let timer: TimerState = serde_json::from_str(r#"{"paused":true,"active_seconds":123,"target_seconds":1800,"started_at":1000,"spans":[{"start":1000,"end":1123}]}"#).unwrap();
    assert_eq!(timer.kind, TimerKind::Study);
    assert_eq!(timer.active_seconds, 123);
    assert_eq!(timer.target_seconds, 1800);
    assert!(timer.finished_at.is_none());
    assert!(timer.label.is_empty());
}

#[test]
fn countdown_stops_exactly_once_and_round_trips_through_backup_and_restart() {
    let (dir, mut engine) = fixture();
    start(&mut engine, TimerKind::CountDown, Some(63));
    for i in 1..=7 {
        engine.tick(1000 + i * 10, 10).unwrap();
    }
    let timer = engine.snapshot().timer;
    assert_eq!(timer.active_seconds, 63);
    assert_eq!(timer.finished_at, Some(1063));
    assert!(timer.paused);
    assert!(engine.snapshot().sessions.is_empty());
    for i in 8..=10 {
        engine.tick(1000 + i * 10, 10).unwrap();
    }
    assert_eq!(engine.snapshot().timer.finished_at, Some(1063));
    assert!(engine.apply(Command::SetPaused(false), 1100).is_err());
    engine
        .apply(Command::Backup(dir.join("backup.sqlite")), 1100)
        .unwrap();
    studyapp_core::verify_backup_file(&dir.join("backup.sqlite")).unwrap();
    drop(engine);
    let mut engine = Engine::open(&dir.join("timers.sqlite"), 1200).unwrap();
    assert_eq!(engine.snapshot().timer.active_seconds, 63);
    assert_eq!(engine.snapshot().timer.finished_at, Some(1063));
    engine.apply(Command::ResetTimer, 1200).unwrap();
    assert_eq!(engine.snapshot().timer.active_seconds, 0);
    assert!(!engine.snapshot().timer.paused);
    engine.tick(1201, 1).unwrap();
    assert_eq!(engine.snapshot().timer.active_seconds, 1);
}

#[test]
fn countup_runs_past_four_hours_and_finish_preserves_elapsed_without_a_session() {
    let (_, mut engine) = fixture();
    start(&mut engine, TimerKind::CountUp, None);
    for i in 1..=1450 {
        engine.tick(1000 + i * 10, 10).unwrap();
    }
    assert_eq!(engine.snapshot().timer.active_seconds, 14500);
    engine.apply(Command::FinishTimer, 15500).unwrap();
    engine.apply(Command::FinishTimer, 15501).unwrap();
    assert_eq!(engine.snapshot().timer.active_seconds, 14500);
    assert_eq!(engine.snapshot().timer.finished_at, Some(15500));
    assert!(engine.snapshot().sessions.is_empty());
}

#[test]
fn generic_timers_keep_targets_when_preferences_change_and_ignore_sleep() {
    for (kind, target) in [(TimerKind::CountUp, None), (TimerKind::CountDown, Some(90))] {
        let (_, mut engine) = fixture();
        start(&mut engine, kind, target);
        let mut settings = engine.snapshot().settings;
        settings.interval_minutes = 25;
        let prepared = engine.prepare_settings(settings.clone()).unwrap();
        engine.install_prepared_settings(prepared);
        assert_eq!(engine.snapshot().timer.target_seconds, target.unwrap_or(0));
        engine
            .apply(Command::SaveSettings(Box::new(settings)), 1001)
            .unwrap();
        engine.tick(2000, 999).unwrap();
        assert_eq!(engine.snapshot().timer.active_seconds, 0);
        engine.tick(2001, 1).unwrap();
        engine.apply(Command::SetPaused(true), 2001).unwrap();
        engine.tick(2101, 10).unwrap();
        assert_eq!(engine.snapshot().timer.active_seconds, 1);
    }
}

#[test]
fn timer_and_appearance_inputs_are_validated_without_mutating_progress() {
    let (_, mut engine) = fixture();
    for target in [0, 59, 10801, u64::MAX] {
        assert!(
            engine
                .apply(
                    Command::StartTimer {
                        kind: TimerKind::CountDown,
                        target_seconds: Some(target),
                        label: String::new()
                    },
                    1000
                )
                .is_err()
        );
    }
    assert!(engine.apply(Command::FinishTimer, 1000).is_err());
    assert_eq!(engine.snapshot().timer.kind, TimerKind::Study);
    let legacy: Settings =
        serde_json::from_str(r#"{"interval_minutes":60,"answer_minutes":5,"review_days":[1,3]}"#)
            .unwrap();
    assert_eq!(*legacy.timer_widget, TimerWidgetSettings::default());
    let mut settings = Settings::default();
    settings.timer_widget.opacity = 29;
    assert!(settings.validate().is_err());
    settings.timer_widget.opacity = 30;
    settings.timer_widget.accent = Some("red;url(x)".into());
    assert!(settings.validate().is_err());
    settings.timer_widget.accent = Some("#227799".into());
    assert!(settings.validate().is_ok());
    settings.timer_widget.countdown_presets = vec![181];
    assert!(settings.validate().is_err());
}

#[test]
fn countup_defensive_day_limit_stops_without_creating_learning_records() {
    let (_, mut engine) = fixture();
    start(&mut engine, TimerKind::CountUp, None);
    for i in 1..=8641 {
        engine.tick(1000 + i * 10, 10).unwrap();
    }
    let timer = engine.snapshot().timer;
    assert_eq!(timer.active_seconds, 86400);
    assert!(timer.paused);
    assert_eq!(timer.finished_at, Some(87400));
    assert!(engine.snapshot().sessions.is_empty());
}

#[test]
fn manual_finish_after_clock_rollback_never_precedes_the_last_active_span() {
    let (_, mut engine) = fixture();
    start(&mut engine, TimerKind::CountDown, Some(90));
    engine.tick(1010, 10).unwrap();
    engine.apply(Command::FinishTimer, 1005).unwrap();
    assert_eq!(engine.snapshot().timer.finished_at, Some(1010));
}

#[test]
fn notes_from_a_generic_timer_do_not_leak_into_the_next_study_segment() {
    let (_, mut engine) = fixture();
    start(&mut engine, TimerKind::CountUp, None);
    engine
        .apply(
            Command::CreateNote {
                title: "通用计时笔记".into(),
                content: "# 通用计时内容".into(),
            },
            1001,
        )
        .unwrap();
    let generic_version = engine.snapshot().versions.last().unwrap().id.clone();
    for second in 1..=120 {
        engine.tick(1000 + second, 1).unwrap();
    }
    engine.apply(Command::FinishTimer, 1120).unwrap();
    assert!(engine.snapshot().sessions.is_empty());
    let mut settings = engine.snapshot().settings;
    settings.interval_minutes = 5;
    engine
        .apply(Command::SaveSettings(Box::new(settings)), 1120)
        .unwrap();
    engine
        .apply(
            Command::StartTimer {
                kind: TimerKind::Study,
                target_seconds: None,
                label: String::new(),
            },
            1121,
        )
        .unwrap();
    engine
        .apply(
            Command::CreateNote {
                title: "专注学习笔记".into(),
                content: "# 专注学习内容".into(),
            },
            1122,
        )
        .unwrap();
    let study_version = engine.snapshot().versions.last().unwrap().id.clone();
    for second in 1..=300 {
        engine.tick(1121 + second, 1).unwrap();
    }
    let snapshot = engine.snapshot();
    assert_eq!(snapshot.sessions.len(), 1);
    assert_eq!(snapshot.sessions[0].version_ids, vec![study_version]);
    assert!(!snapshot.sessions[0].version_ids.contains(&generic_version));
    assert_eq!(snapshot.timer.kind, TimerKind::Study);
    assert_eq!(snapshot.timer.active_seconds, 0);
}
