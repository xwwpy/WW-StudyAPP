//! Notes are read exclusively from the already-verified manifest payload.
use semver::Version;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseNotes {
    pub version: Version,
    pub notes: String,
}

pub fn parse(root: &serde_json::Value, target: &Version) -> Result<Vec<ReleaseNotes>, String> {
    let Some(entries) = root.get("history") else {
        return Ok(Vec::new());
    };
    let entries = entries.as_array().ok_or("Invalid release history")?;
    if entries.is_empty() || entries.len() > 512 {
        return Err("Invalid release history length".into());
    }
    let mut versions = std::collections::HashSet::new();
    let mut history = Vec::new();
    for entry in entries {
        let version = Version::parse(entry["version"].as_str().ok_or("Missing history version")?)
            .map_err(|_| "Invalid history version")?;
        if !version.pre.is_empty()
            || !version.build.is_empty()
            || &version > target
            || !versions.insert(version.clone())
        {
            return Err("Invalid or duplicate history version".into());
        }
        history.push(ReleaseNotes {
            version,
            notes: entry["notes"]
                .as_str()
                .ok_or("Missing history notes")?
                .to_owned(),
        });
    }
    if !versions.contains(target) {
        return Err("History omits target release".into());
    }
    history.sort_by(|a, b| b.version.cmp(&a.version));
    Ok(history)
}

/// The only presentation boundary: callers get one entry per eligible release,
/// never the manifest's compatibility aggregate as an unclassified Markdown blob.
pub fn releases_since(
    history: &[ReleaseNotes],
    fallback: &str,
    current: &str,
    target: &Version,
) -> Vec<ReleaseNotes> {
    let Ok(current) = Version::parse(current) else {
        return Vec::new();
    };
    if &current >= target {
        return Vec::new();
    }
    let legacy;
    let source = if history.is_empty() {
        legacy = legacy_releases(fallback, target);
        &legacy
    } else {
        history
    };
    let mut selected = source
        .iter()
        .filter(|entry| {
            entry.version > current
                && &entry.version <= target
                && entry.version.pre.is_empty()
                && entry.version.build.is_empty()
        })
        .cloned()
        .collect::<Vec<_>>();
    selected.sort_by(|a, b| b.version.cmp(&a.version));
    selected.dedup_by(|a, b| a.version == b.version);
    selected
}

fn legacy_releases(notes: &str, target: &Version) -> Vec<ReleaseNotes> {
    if !notes.starts_with("## v") {
        return vec![ReleaseNotes {
            version: target.clone(),
            notes: notes.to_owned(),
        }];
    }
    // Compatibility with the exact aggregate format of older signed envelopes.
    // If it cannot be classified safely, do not label all history as the target.
    let entries = notes
        .split("\n\n---\n\n")
        .take(513)
        .map(|section| {
            let (version, notes) = section.strip_prefix("## v")?.split_once("\n\n")?;
            Some(ReleaseNotes {
                version: Version::parse(version).ok()?,
                notes: notes.to_owned(),
            })
        })
        .collect::<Option<Vec<_>>>();
    match entries {
        Some(entries)
            if entries.len() <= 512 && entries.iter().any(|entry| &entry.version == target) =>
        {
            entries
        }
        _ => vec![ReleaseNotes {
            version: target.clone(),
            notes: "本版本未提供独立更新说明，请查看发布页面。".into(),
        }],
    }
}

pub fn notes_since(
    history: &[ReleaseNotes],
    fallback: &str,
    current: &str,
    target: &Version,
) -> String {
    releases_since(history, fallback, current, target)
        .iter()
        .map(|entry| format!("## v{}\n\n{}", entry.version, entry.notes))
        .collect::<Vec<_>>()
        .join("\n\n---\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn skipped_releases_are_complete_sorted_and_exclude_already_read_notes() {
        let target = Version::parse("0.2.4").unwrap();
        let root = serde_json::json!({"history":[
            {"version":"0.2.2","notes":"old"},
            {"version":"0.2.4","notes":"four"},
            {"version":"0.2.3","notes":"three"}
        ]});
        let entries = parse(&root, &target).unwrap();
        assert_eq!(
            notes_since(&entries, "", "0.2.2", &target),
            "## v0.2.4\n\nfour\n\n---\n\n## v0.2.3\n\nthree"
        );
        assert!(!notes_since(&entries, "", "0.2.3", &target).contains("three"));
        assert!(notes_since(&entries, "", "0.2.4", &target).is_empty());
        assert_eq!(
            notes_since(&[], "legacy", "0.2.2", &target),
            "## v0.2.4

legacy"
        );
    }
    #[test]
    fn structured_range_uses_semver_and_does_not_leak_legacy_aggregate_history() {
        let target = Version::parse("0.10.0").unwrap();
        let legacy = "## v0.10.0

new

---

## v0.9.0

installed";
        let selected = releases_since(&[], legacy, "0.9.0", &target);
        assert_eq!(
            selected,
            vec![ReleaseNotes {
                version: target.clone(),
                notes: "new".into()
            }]
        );
        assert!(releases_since(&[], legacy, "0.10.0", &target).is_empty());
        assert!(releases_since(&[], legacy, "broken", &target).is_empty());
        assert_eq!(
            releases_since(&[], "legacy body", "0.9.0", &target)[0].notes,
            "legacy body"
        );
        assert!(
            !releases_since(
                &[],
                "## vbroken

old history",
                "0.9.0",
                &target
            )[0]
            .notes
            .contains("old history")
        );
        let history = vec![
            ReleaseNotes {
                version: Version::parse("0.9.0").unwrap(),
                notes: "old".into(),
            },
            ReleaseNotes {
                version: Version::parse("0.10.1").unwrap(),
                notes: "future".into(),
            },
            ReleaseNotes {
                version: target.clone(),
                notes: "target".into(),
            },
        ];
        assert_eq!(
            releases_since(&history, "aggregate", "0.9.0", &target),
            vec![history[2].clone()]
        );
    }

    #[test]
    fn malformed_history_never_silently_degrades_to_latest_only() {
        let target = Version::parse("0.2.4").unwrap();
        for rows in [
            serde_json::json!([]),
            serde_json::json!([{"version":"0.2.3","notes":"missing target"}]),
            serde_json::json!([{"version":"0.2.4-beta.1","notes":"prerelease"}]),
            serde_json::json!([{"version":"0.2.5","notes":"future"}]),
            serde_json::json!([{"version":"0.2.4","notes":"a"},{"version":"0.2.4","notes":"b"}]),
        ] {
            assert!(parse(&serde_json::json!({"history":rows}), &target).is_err());
        }
    }
}
