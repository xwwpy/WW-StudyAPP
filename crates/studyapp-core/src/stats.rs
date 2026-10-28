use crate::*;
use chrono::{Duration, NaiveDate};
/// Group confirmed activity by a fixed UTC offset; spans are split at local midnight.
pub fn daily_stats(
    snapshot: &Snapshot,
    offset_seconds: i32,
    end_date: &str,
    days: usize,
) -> Vec<DayStat> {
    let Some(end) = NaiveDate::parse_from_str(end_date, "%Y-%m-%d").ok() else {
        return vec![];
    };
    if days == 0 || days > 36600 {
        return vec![];
    }
    let Some(start) = end.checked_sub_signed(Duration::days((days - 1) as i64)) else {
        return vec![];
    };
    let offset = i64::from(offset_seconds);
    let mut stats: Vec<DayStat> = (0..days)
        .filter_map(|n| start.checked_add_signed(Duration::days(n as i64)))
        .map(|date| DayStat {
            date: date.format("%Y-%m-%d").to_string(),
            learned_seconds: 0,
            learned_count: 0,
            missed_count: 0,
            review_count: 0,
        })
        .collect();
    let start_stamp = start
        .and_hms_opt(0, 0, 0)
        .expect("valid midnight")
        .and_utc()
        .timestamp();
    let day_index = |stamp: i64| -> Option<usize> {
        let delta = stamp.checked_add(offset)?.checked_sub(start_stamp)?;
        let i = delta.div_euclid(86400);
        (i >= 0 && i < days as i64).then_some(i as usize)
    };
    for session in &snapshot.sessions {
        if let Some(i) = day_index(session.ended_at) {
            match session.status {
                SessionStatus::Learned => stats[i].learned_count += 1,
                SessionStatus::Missed => stats[i].missed_count += 1,
                SessionStatus::Pending => {}
            }
        }
        if session.status != SessionStatus::Learned {
            continue;
        }
        for span in &session.spans {
            // Clip to requested window before splitting; malformed / huge spans cannot cause an unbounded loop.
            let lower = start_stamp.saturating_sub(offset);
            let upper = lower.saturating_add(days as i64 * 86400);
            let mut cursor = span.start.max(lower);
            let end = span.end.min(upper);
            while cursor < end {
                let Some(i) = day_index(cursor) else {
                    break;
                };
                let boundary = lower.saturating_add((i as i64 + 1) * 86400).min(end);
                if boundary <= cursor {
                    break;
                }
                stats[i].learned_seconds = stats[i]
                    .learned_seconds
                    .saturating_add((boundary - cursor) as u64);
                cursor = boundary;
            }
        }
    }
    for event in &snapshot.review_events {
        if let Some(i) = day_index(event.completed_at) {
            stats[i].review_count += 1;
        }
    }
    stats
}
