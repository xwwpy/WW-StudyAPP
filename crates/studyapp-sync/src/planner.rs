use crate::*;
use std::collections::{BTreeMap, BTreeSet};

/// Reads an immutable snapshot and returns per-note transactions. No baseline is
/// persisted here. The caller must verify `expected_local_hash`, execute the
/// action, and only then persist `next_baseline`. Preserve a conflict copy before
/// receiving its remote original. An external-source copy never edits the source.
/// Excluded IDs are neither uploaded nor read/imported from the remote listing.
pub fn plan_sync(
    provider: &dyn Provider,
    notes: &[SyncNote],
    baselines: &BTreeMap<String, Baseline>,
    excluded: &BTreeSet<String>,
) -> Result<Vec<NotePlan>> {
    if notes.len() > MAX_ENTRIES {
        return Err(SyncError::LimitExceeded);
    }
    let mut local = BTreeMap::new();
    let mut bytes = 0usize;
    for note in notes {
        if excluded.contains(&note.id) {
            continue;
        }
        validate_id(&note.id)?;
        validate_content(&note.content)?;
        if content_hash(&note.content) != note.hash {
            return Err(SyncError::InvalidContent);
        }
        bytes = bytes
            .checked_add(note.content.len())
            .ok_or(SyncError::LimitExceeded)?;
        if bytes > MAX_SYNC_BYTES {
            return Err(SyncError::LimitExceeded);
        }
        if local.insert(note.id.as_str(), note).is_some() {
            return Err(SyncError::InvalidId);
        }
    }
    let remote_ids = provider.list()?;
    if remote_ids.len() > MAX_ENTRIES {
        return Err(SyncError::LimitExceeded);
    }
    let mut ids: BTreeSet<String> = local.keys().map(|id| (*id).to_owned()).collect();
    for id in remote_ids {
        validate_id(&id)?;
        if !excluded.contains(&id) {
            ids.insert(id);
        }
    }
    if ids.len() > MAX_ENTRIES {
        return Err(SyncError::LimitExceeded);
    }
    let mut plans = Vec::new();
    for id in ids {
        let note = local.get(id.as_str()).copied();
        let remote = provider.read(&id)?;
        if let Some(remote) = &remote {
            validate_content(&remote.content)?;
            if content_hash(&remote.content) != remote.hash {
                return Err(SyncError::InvalidContent);
            }
            bytes = bytes
                .checked_add(remote.content.len())
                .ok_or(SyncError::LimitExceeded)?;
            if bytes > MAX_SYNC_BYTES {
                return Err(SyncError::LimitExceeded);
            }
        }
        let (action, local_hash, remote_hash) = match (note, remote) {
            (None, None) => continue,
            (Some(note), None) => (
                SyncAction::Push {
                    content: note.content.clone(),
                    expected_revision: None,
                },
                note.hash.clone(),
                note.hash.clone(),
            ),
            (None, Some(remote)) => (
                SyncAction::ImportRemote {
                    content: remote.content,
                },
                remote.hash.clone(),
                remote.hash,
            ),
            (Some(note), Some(remote)) => {
                let baseline = baselines.get(&id);
                let local_changed = baseline.is_none_or(|b| b.local_hash != note.hash);
                let remote_changed = baseline.is_none_or(|b| b.remote_hash != remote.hash);
                if note.hash == remote.hash || (!local_changed && !remote_changed) {
                    (SyncAction::Unchanged, note.hash.clone(), remote.hash)
                } else if !remote_changed {
                    let revision = remote.revision.ok_or(SyncError::MissingRevision)?;
                    (
                        SyncAction::Push {
                            content: note.content.clone(),
                            expected_revision: Some(revision),
                        },
                        note.hash.clone(),
                        note.hash.clone(),
                    )
                } else if !note.managed {
                    (
                        SyncAction::CopyExternal {
                            content: remote.content,
                        },
                        note.hash.clone(),
                        remote.hash,
                    )
                } else if local_changed {
                    (
                        SyncAction::Conflict {
                            local_content: note.content.clone(),
                            remote_content: remote.content,
                        },
                        remote.hash.clone(),
                        remote.hash,
                    )
                } else {
                    (
                        SyncAction::Pull {
                            content: remote.content,
                        },
                        remote.hash.clone(),
                        remote.hash,
                    )
                }
            }
        };
        plans.push(NotePlan {
            id,
            expected_local_hash: note.map(|n| n.hash.clone()),
            action,
            next_baseline: Baseline {
                local_hash,
                remote_hash,
            },
        });
    }
    Ok(plans)
}
