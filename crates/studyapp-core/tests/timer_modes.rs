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
    assert!(
        engine
            .apply(
                Command::StartTimer {
                    kind: TimerKind::CountDown,
                    target_seconds: Some(0),
                    label: String::new(),
                },
                1000
            )
            .is_err()
    );
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
    settings.timer_widget.countdown_presets = vec![181, 1500, u64::MAX / 60];
    assert!(settings.validate().is_ok());
    settings.timer_widget.countdown_presets = vec![u64::MAX / 60 + 1];
    assert!(settings.validate().is_err());
}

#[test]
fn countup_continues_past_a_day_and_survives_backup_restore_and_restart() {
    let (dir, mut engine) = fixture();
    start(&mut engine, TimerKind::CountUp, None);
    for i in 1..=8641 {
        engine.tick(1000 + i * 10, 10).unwrap();
    }
    let timer = engine.snapshot().timer;
    assert_eq!(timer.active_seconds, 86410);
    assert!(!timer.paused);
    assert!(timer.finished_at.is_none());
    assert!(engine.snapshot().sessions.is_empty());
    let backup = dir.join("beyond-day.sqlite");
    engine
        .apply(Command::Backup(backup.clone()), 87410)
        .unwrap();
    studyapp_core::verify_backup_file(&backup).unwrap();
    engine.apply(Command::Restore(backup), 87500).unwrap();
    assert_eq!(engine.snapshot().timer.active_seconds, 86410);
    assert!(engine.snapshot().timer.finished_at.is_none());
    drop(engine);
    let mut engine = Engine::open(&dir.join("timers.sqlite"), 87600).unwrap();
    assert_eq!(engine.snapshot().timer.active_seconds, 86410);
    engine.apply(Command::SetPaused(false), 87600).unwrap();
    engine.tick(87601, 1).unwrap();
    assert_eq!(engine.snapshot().timer.active_seconds, 86411);
    engine.apply(Command::FinishTimer, 87601).unwrap();
    assert_eq!(engine.snapshot().timer.finished_at, Some(87601));
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

#[test]
fn countdown_has_no_artificial_range_and_preserves_large_targets() {
    for target in [1, 59, 10801, 90061, u64::MAX] {
        let (dir, mut engine) = fixture();
        start(&mut engine, TimerKind::CountDown, Some(target));
        engine.tick(1010, 10).unwrap();
        let before = engine.snapshot().timer;
        assert_eq!(before.active_seconds, target.min(10));
        assert_eq!(before.finished_at.is_some(), target <= 10);
        let backup = dir.join("large-countdown.sqlite");
        engine.apply(Command::Backup(backup.clone()), 1010).unwrap();
        studyapp_core::verify_backup_file(&backup).unwrap();
        engine.apply(Command::Restore(backup), 1020).unwrap();
        assert_eq!(engine.snapshot().timer.target_seconds, target);
        assert_eq!(
            engine.snapshot().timer.active_seconds,
            before.active_seconds
        );
        assert_eq!(engine.snapshot().timer.finished_at, before.finished_at);
        drop(engine);
        let engine = Engine::open(&dir.join("timers.sqlite"), 1030).unwrap();
        assert_eq!(engine.snapshot().timer.target_seconds, target);
        assert_eq!(
            engine.snapshot().timer.active_seconds,
            before.active_seconds
        );
    }
}

#[test]
fn countdown_input_and_presets_share_checked_minute_conversion() {
    use studyapp_core::{countdown_seconds, parse_countdown_minutes};
    for (input, minutes) in [("1", 1), (" 000181 ", 181), ("1500", 1500)] {
        assert_eq!(parse_countdown_minutes(input).unwrap(), minutes);
        assert_eq!(countdown_seconds(minutes).unwrap(), minutes * 60);
    }
    let largest = u64::MAX / 60;
    assert_eq!(
        parse_countdown_minutes(&largest.to_string()).unwrap(),
        largest
    );
    assert_eq!(countdown_seconds(largest).unwrap(), largest * 60);
    assert!(countdown_seconds(largest + 1).is_err());
    for invalid in ["", "0", "-1", "+1", "1.5", "1e3", "1 0", "１８１"] {
        assert!(parse_countdown_minutes(invalid).is_err(), "{invalid}");
    }
    assert!(parse_countdown_minutes(&(largest + 1).to_string()).is_err());
    assert!(parse_countdown_minutes("18446744073709551616").is_err());
    let settings = TimerWidgetSettings {
        countdown_presets: vec![181, largest],
        ..Default::default()
    };
    settings.validate().unwrap();
    let decoded: TimerWidgetSettings =
        serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
    assert_eq!(decoded, settings);
}

#[test]
fn study_timer_accepts_positive_minutes_below_and_above_legacy_ranges() {
    for minutes in [1, 4, 31, 241, 1500, u32::MAX] {
        let (dir, mut engine) = fixture();
        engine
            .apply(
                Command::SaveLearningSettings {
                    interval_minutes: minutes,
                    answer_minutes: minutes,
                    review_days: vec![1, 3],
                },
                1000,
            )
            .unwrap();
        let snapshot = engine.snapshot();
        assert_eq!(snapshot.settings.interval_minutes, minutes);
        assert_eq!(snapshot.settings.answer_minutes, minutes);
        assert_eq!(snapshot.timer.target_seconds, u64::from(minutes) * 60);
        let backup = dir.join("positive-minutes.sqlite");
        engine.apply(Command::Backup(backup.clone()), 1000).unwrap();
        studyapp_core::verify_backup_file(&backup).unwrap();
        drop(engine);
        let reopened = Engine::open(&dir.join("timers.sqlite"), 1000).unwrap();
        assert_eq!(reopened.snapshot().settings, snapshot.settings);
        assert_eq!(
            reopened.snapshot().timer.target_seconds,
            snapshot.timer.target_seconds
        );
    }
}

#[test]
fn one_minute_study_finishes_with_an_unrestricted_answer_deadline() {
    let (_, mut engine) = fixture();
    engine
        .apply(
            Command::SaveLearningSettings {
                interval_minutes: 1,
                answer_minutes: 1500,
                review_days: vec![1, 3],
            },
            1000,
        )
        .unwrap();
    engine.apply(Command::SetPaused(false), 1000).unwrap();
    for i in 1..=6 {
        engine.tick(1000 + i * 10, 10).unwrap();
    }
    let snapshot = engine.snapshot();
    assert_eq!(snapshot.sessions.len(), 1);
    assert_eq!(snapshot.sessions[0].active_seconds, 60);
    assert_eq!(snapshot.sessions[0].ended_at, 1060);
    assert_eq!(snapshot.sessions[0].deadline, 1060 + 1500 * 60);
    assert_eq!(snapshot.timer.target_seconds, 60);
    assert_eq!(snapshot.timer.active_seconds, 0);
}

#[test]
fn study_minute_representation_rejects_zero_and_overflow_without_mutation() {
    let (_, mut engine) = fixture();
    let original = engine.snapshot().settings;
    for (interval_minutes, answer_minutes) in [(0, 1), (1, 0)] {
        assert!(
            engine
                .apply(
                    Command::SaveLearningSettings {
                        interval_minutes,
                        answer_minutes,
                        review_days: vec![1, 3],
                    },
                    1000
                )
                .is_err()
        );
        assert_eq!(engine.snapshot().settings, original);
    }
    for field in ["interval_minutes", "answer_minutes"] {
        let mut value = serde_json::to_value(&original).unwrap();
        value[field] = serde_json::json!(u64::from(u32::MAX) + 1);
        assert!(serde_json::from_value::<Settings>(value).is_err());
    }
}

#[test]
fn oversized_answer_deadline_fails_without_advancing_timer_or_creating_a_session() {
    let (_, mut engine) = fixture();
    engine
        .apply(
            Command::SaveLearningSettings {
                interval_minutes: 1,
                answer_minutes: u32::MAX,
                review_days: vec![1, 3],
            },
            1000,
        )
        .unwrap();
    engine.apply(Command::SetPaused(false), 1000).unwrap();
    for i in 1..=5 {
        engine.tick(1000 + i * 10, 10).unwrap();
    }
    let before = serde_json::to_value(engine.snapshot().timer).unwrap();
    assert!(engine.tick(i64::MAX - 5, 10).is_err());
    assert_eq!(
        serde_json::to_value(engine.snapshot().timer).unwrap(),
        before
    );
    assert!(engine.snapshot().sessions.is_empty());
    engine.tick(1060, 10).unwrap();
    assert_eq!(engine.snapshot().sessions.len(), 1);
    assert_eq!(
        engine.snapshot().sessions[0].deadline,
        1060 + i64::from(u32::MAX) * 60
    );
}
