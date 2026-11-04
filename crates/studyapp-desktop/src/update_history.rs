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

pub fn notes_since(
    history: &[ReleaseNotes],
    fallback: &str,
    current: &str,
    target: &Version,
) -> String {
    let Ok(current) = Version::parse(current) else {
        return fallback.to_owned();
    };
    if history.is_empty() {
        return fallback.to_owned();
    }
    history
        .iter()
        .filter(|entry| entry.version > current && &entry.version <= target)
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
        assert_eq!(notes_since(&[], "legacy", "0.2.2", &target), "legacy");
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
