use chrono::{Local, NaiveDateTime, TimeZone};

/// Presentation only; canonical paths must remain unchanged for IO and identity.
pub fn display_path(path: &str) -> String {
    if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{unc}");
    }
    if let Some(drive) = path.strip_prefix(r"\\?\")
        && drive.as_bytes().get(1) == Some(&b':')
        && drive
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphabetic)
    {
        return drive.to_owned();
    }
    path.to_owned()
}

/// Bound each visual diff row without dropping any content. CJK/emoji receive
/// two columns, and tabs become four spaces. Cached by the view, not each frame.
pub fn diff_rows(diff: &str, columns: usize) -> Vec<(String, char)> {
    let columns = columns.max(4);
    let mut rows = Vec::new();
    for line in diff.lines() {
        let marker = line.chars().next().unwrap_or(' ');
        let mut row = String::new();
        let mut width = 0;
        for ch in line.chars() {
            let size = if ch == '\t' {
                4
            } else if ch.is_ascii() {
                1
            } else {
                2
            };
            if width + size > columns && !row.is_empty() {
                rows.push((std::mem::take(&mut row), marker));
                width = 0;
            }
            if ch == '\t' {
                row.push_str("    ");
            } else {
                row.push(ch);
            }
            width += size;
        }
        rows.push((row, marker));
    }
    rows
}

#[cfg(test)]
mod path_and_diff_tests {
    use super::*;
    #[test]
    fn windows_paths_are_displayed_without_changing_other_paths() {
        assert_eq!(display_path(r"\\?\C:\学习\笔记.md"), r"C:\学习\笔记.md");
        assert_eq!(
            display_path(r"\\?\UNC\nas\share\笔记.md"),
            r"\\nas\share\笔记.md"
        );
        for path in [
            r"\\nas\share",
            r"C:\notes",
            "/home/notes",
            r"\\?\Volume{abc}\",
        ] {
            assert_eq!(display_path(path), path);
        }
    }
    #[test]
    fn long_diffs_keep_all_rows_and_unicode_without_truncation() {
        let text = format!("+{}\n-{}\n", "中文🧠a".repeat(5000), "tail\n".repeat(1600));
        let rows = diff_rows(&text, 28);
        assert!(rows.len() > 1500);
        assert_eq!(
            rows.iter().map(|r| r.0.as_str()).collect::<String>(),
            text.replace('\n', "")
        );
        assert!(rows.iter().take(100).all(|r| r.1 == '+'));
        assert!(rows.iter().all(|r| r.0.chars().count() <= 28));
    }
}

pub fn safe_markdown_html(source: &str) -> String {
    use pulldown_cmark::Parser;
    safe_markdown_events(Parser::new_ext(source, crate::markdown::markdown_options()))
}

pub(crate) fn safe_markdown_events<'a>(
    events: impl IntoIterator<Item = pulldown_cmark::Event<'a>>,
) -> String {
    use pulldown_cmark::{Event, Tag, TagEnd, html};
    let events = events.into_iter().filter_map(|event| match event {
        Event::Html(_) | Event::InlineHtml(_) => None,
        Event::Start(Tag::Image { .. }) => Some(Event::Text("[图片：".into())),
        Event::End(TagEnd::Image) => Some(Event::Text("]".into())),
        Event::Start(Tag::Link { .. }) | Event::End(TagEnd::Link) => None,
        other => Some(other),
    });
    let mut rendered = String::new();
    html::push_html(&mut rendered, events);
    rendered
}

pub fn parse_local_time(value: &str) -> anyhow::Result<i64> {
    let value = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M")?;
    Local
        .from_local_datetime(&value)
        .single()
        .map(|time| time.timestamp())
        .ok_or_else(|| anyhow::anyhow!("该时间处于夏令时切换范围，请选择明确的本地时间"))
}

pub fn stamp(value: i64) -> String {
    Local
        .timestamp_opt(value, 0)
        .single()
        .map(|time| time.format("%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "无效时间".into())
}

pub fn duration(seconds: u64) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

/// Calendar-day boundaries come from the timezone for each date, including
/// 23/25-hour DST days; today's fixed offset cannot describe historical days.
pub fn calendar_stats<Tz: TimeZone>(
    snapshot: &studyapp_core::Snapshot,
    timezone: &Tz,
    end: chrono::NaiveDate,
    days: usize,
) -> Vec<studyapp_core::DayStat> {
    use studyapp_core::{DayStat, SessionStatus};
    let mut output = Vec::new();
    for ago in (0..days).rev() {
        let Some(date) = end.checked_sub_signed(chrono::Duration::days(ago as i64)) else {
            continue;
        };
        let start_of = |date: chrono::NaiveDate| {
            (0..24).find_map(|hour| {
                timezone
                    .from_local_datetime(&date.and_hms_opt(hour, 0, 0)?)
                    .earliest()
                    .map(|t| t.timestamp())
            })
        };
        let mut stat = DayStat {
            date: date.to_string(),
            learned_seconds: 0,
            learned_count: 0,
            missed_count: 0,
            review_count: 0,
        };
        if let (Some(start), Some(end)) = (start_of(date), date.succ_opt().and_then(start_of)) {
            for session in &snapshot.sessions {
                if (start..end).contains(&session.ended_at) {
                    match session.status {
                        SessionStatus::Learned => stat.learned_count += 1,
                        SessionStatus::Missed => stat.missed_count += 1,
                        _ => {}
                    }
                }
                if session.status == SessionStatus::Learned {
                    stat.learned_seconds += session
                        .spans
                        .iter()
                        .map(|span| (span.end.min(end) - span.start.max(start)).max(0) as u64)
                        .sum::<u64>();
                }
            }
            stat.review_count = snapshot
                .review_events
                .iter()
                .filter(|e| (start..end).contains(&e.completed_at))
                .count();
        }
        output.push(stat);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untrusted_markdown_cannot_load_resources_or_execute_links() {
        let html = safe_markdown_html(
            "# 笔记\n\n![隐私](https://tracker.invalid/a.png)\n\n<script>alert(1)</script>\n\n[危险](file:///private.txt)\n\n**学习**",
        );
        assert!(!html.contains("<script"));
        assert!(!html.contains("https://tracker"));
        assert!(!html.contains("file:///"));
        assert!(html.contains("<h1>笔记</h1>"));
        assert!(html.contains("<strong>学习</strong>"));
    }

    #[test]
    fn markdown_keeps_code_and_tables_without_loading_html_images() {
        let html = safe_markdown_html(
            "<img src='https://tracker.invalid'>\n\n```rust\nlet x = 1;\n```\n\n| 项目 | 次数 |\n|---|---|\n| 复习 | 3 |\n",
        );
        assert!(!html.contains("<img"));
        assert!(html.contains("<table>"));
        assert!(html.contains("let x = 1;"));
    }

    #[test]
    fn timestamp_validation_rejects_impossible_dates() {
        assert!(parse_local_time("2026-02-30 12:00").is_err());
        assert!(parse_local_time("not a date").is_err());
        assert_eq!(duration(3661), "01:01:01");
    }

    #[test]
    fn local_calendar_uses_twenty_three_hour_dst_day() {
        use studyapp_core::{Command, Engine};
        let directory = tempfile::tempdir().unwrap().keep();
        let zone = chrono_tz::America::New_York;
        let start = zone
            .with_ymd_and_hms(2026, 3, 8, 0, 0, 0)
            .unwrap()
            .timestamp();
        let end = zone
            .with_ymd_and_hms(2026, 3, 9, 0, 0, 0)
            .unwrap()
            .timestamp();
        let mut engine = Engine::open(&directory.join("dst.sqlite3"), end).unwrap();
        engine
            .apply(
                Command::Manual {
                    started_at: start,
                    minutes: 23 * 60,
                    summary: "跨日记录".into(),
                    note_ids: vec![],
                    version_ids: None,
                },
                end,
            )
            .unwrap();
        let stats = calendar_stats(
            &engine.snapshot(),
            &zone,
            chrono::NaiveDate::from_ymd_opt(2026, 3, 9).unwrap(),
            3,
        );
        assert_eq!(stats[1].learned_seconds, 23 * 3600);
        assert_eq!(stats[2].learned_seconds, 0);
        assert_eq!(stats[2].learned_count, 1);
    }
}
