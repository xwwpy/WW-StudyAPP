//! Snapshot-derived library metadata. Pointer-driven renders only read this index.
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
    sync::Arc,
};
use studyapp_core::{LibrarySort, Note, NoteVersion, SessionStatus, Snapshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StudyStatus {
    Added,
    Modified,
    Recorded,
}
impl StudyStatus {
    pub fn from_hashes(latest: Option<&str>, recorded: Option<&str>) -> Self {
        match (latest, recorded) {
            (Some(a), Some(b)) if a == b => Self::Recorded,
            (_, Some(_)) => Self::Modified,
            _ => Self::Added,
        }
    }
    pub fn from_history(
        latest: Option<&str>,
        recorded: Option<&str>,
        baseline: Option<&str>,
    ) -> Self {
        if recorded.is_some() {
            return Self::from_hashes(latest, recorded);
        }
        match (latest, baseline) {
            (Some(current), Some(original)) if current != original => Self::Modified,
            _ => Self::Added,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Added => "A · 尚未记录",
            Self::Modified => "M · 修改未记录",
            Self::Recorded => "已记录",
        }
    }
}
pub fn source_label(note: &Note) -> &'static str {
    if note.detached {
        "已取消管理"
    } else if note.managed {
        "应用管理副本"
    } else {
        "外部跟踪 · 只读"
    }
}
pub fn compare_entries(sort: LibrarySort, a: (&str, i64, &str), b: (&str, i64, &str)) -> Ordering {
    let primary = match sort {
        LibrarySort::NameAscending => a.0.to_lowercase().cmp(&b.0.to_lowercase()),
        LibrarySort::NameDescending => b.0.to_lowercase().cmp(&a.0.to_lowercase()),
        LibrarySort::Newest => b.1.cmp(&a.1),
        LibrarySort::Oldest => a.1.cmp(&b.1),
    };
    primary.then_with(|| a.2.cmp(b.2))
}
#[derive(Clone)]
pub struct NoteMetadata {
    pub position: usize,
    pub version_count: usize,
    pub status: StudyStatus,
    pub tip: String,
    pub updated_stamp: String,
    title_key: String,
    search: String,
    ancestors: Vec<String>,
}
#[derive(Default)]
pub struct LibraryIndex {
    notes: HashMap<String, NoteMetadata>,
    order: [Vec<usize>; 4],
    folders: HashMap<String, i64>,
    direct_folders: HashSet<String>,
    relations: HashMap<String, FolderRelation>,
    versions: Option<Arc<Vec<NoteVersion>>>,
    signature: u64,
    /// Number of content bodies normalized, for deterministic drag regressions.
    pub content_bodies_indexed: usize,
}

#[derive(Default)]
struct FolderRelation {
    parent: Option<String>,
    ancestors: Vec<String>,
    subtree_height: usize,
}
fn signature(snapshot: &Snapshot) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    snapshot.data_path.hash(&mut h);
    for n in &snapshot.notes {
        (
            &n.id,
            &n.title,
            &n.path,
            &n.folder_id,
            n.revision,
            n.updated_at,
            n.detached,
            n.managed,
            n.missing,
        )
            .hash(&mut h);
    }
    for s in &snapshot.sessions {
        (
            &s.id,
            s.revision,
            s.updated_at,
            &s.version_ids,
            &s.note_ids,
            s.ended_at,
            std::mem::discriminant(&s.status),
        )
            .hash(&mut h);
    }
    for r in &snapshot.reviews {
        (&r.id, r.revision, r.last_review_at, r.next_review_at).hash(&mut h);
    }
    for f in &snapshot.folders {
        (&f.id, &f.name, &f.parent_id).hash(&mut h);
    }
    h.finish()
}
impl LibraryIndex {
    pub fn needs_update(&self, snapshot: &Snapshot) -> bool {
        self.versions
            .as_ref()
            .is_none_or(|v| !Arc::ptr_eq(v, &snapshot.versions))
            || self.signature != signature(snapshot)
    }
    pub fn ordered_notes(&self, sort: LibrarySort) -> &[usize] {
        &self.order[match sort {
            LibrarySort::NameAscending => 0,
            LibrarySort::NameDescending => 1,
            LibrarySort::Newest => 2,
            LibrarySort::Oldest => 3,
        }]
    }
    pub fn note(&self, id: &str) -> Option<&NoteMetadata> {
        self.notes.get(id)
    }
    pub fn matches(&self, id: &str, query: &str) -> bool {
        query.is_empty() || self.notes.get(id).is_some_and(|m| m.search.contains(query))
    }
    pub fn matching_folders(&self, query: &str) -> HashSet<String> {
        self.notes
            .values()
            .filter(|m| m.search.contains(query))
            .flat_map(|m| m.ancestors.iter().cloned())
            .collect()
    }
    /// Search temporarily reveals matching branches; otherwise collapsed
    /// ancestors prevent GPUI row construction, not only painting.
    pub fn note_visible(&self, id: &str, collapsed: &HashSet<String>, query: &str) -> bool {
        !query.is_empty()
            || self
                .notes
                .get(id)
                .is_some_and(|m| !m.ancestors.iter().any(|id| collapsed.contains(id)))
    }
    pub fn folder_updated(&self, id: &str) -> i64 {
        self.folders.get(id).copied().unwrap_or(0)
    }
    pub fn folder_has_notes(&self, id: &str) -> bool {
        self.direct_folders.contains(id)
    }
    pub fn folder_exists(&self, id: &str) -> bool {
        self.relations.contains_key(id)
    }
    /// Snapshot-prepared relationships make pointer hit tests bounded by depth.
    pub fn can_move_folder(&self, moving: &str, destination: Option<&str>) -> bool {
        let Some(source) = self.relations.get(moving) else {
            return false;
        };
        if source.parent.as_deref() == destination {
            return false;
        }
        match destination {
            None => source.subtree_height <= studyapp_core::MAX_FOLDER_DEPTH,
            Some(id) => self.relations.get(id).is_some_and(|target| {
                id != moving
                    && !target.ancestors.iter().any(|ancestor| ancestor == moving)
                    && target.ancestors.len() + 1 + source.subtree_height
                        <= studyapp_core::MAX_FOLDER_DEPTH
            }),
        }
    }
    pub fn build(snapshot: &Snapshot) -> Self {
        let mut result = Self {
            signature: signature(snapshot),
            versions: Some(snapshot.versions.clone()),
            ..Default::default()
        };
        let mut latest = HashMap::new();
        let mut baseline = HashMap::new();
        let mut counts = HashMap::new();
        for v in snapshot.versions.iter() {
            *counts.entry(v.note_id.as_str()).or_insert(0usize) += 1;
            baseline
                .entry(v.note_id.as_str())
                .or_insert(v.hash.as_str());
            latest.insert(v.note_id.as_str(), v);
        }
        let mut recorded_ids = HashSet::new();
        let mut learned: HashMap<&str, i64> = HashMap::new();
        for session in snapshot
            .sessions
            .iter()
            .filter(|s| s.status == SessionStatus::Learned)
        {
            for id in &session.note_ids {
                learned
                    .entry(id)
                    .and_modify(|t| *t = (*t).max(session.ended_at))
                    .or_insert(session.ended_at);
            }
            recorded_ids.extend(session.version_ids.iter().map(String::as_str));
        }
        // Follow canonical version order, so a later session studying an old
        // historical snapshot cannot mark newer recorded content as modified.
        let mut recorded = HashMap::new();
        for version in snapshot
            .versions
            .iter()
            .filter(|v| recorded_ids.contains(v.id.as_str()))
        {
            recorded.insert(version.note_id.as_str(), version.hash.as_str());
        }
        let mut reviewed = HashMap::<&str, i64>::new();
        let mut due = HashMap::<&str, i64>::new();
        for review in &snapshot.reviews {
            for id in &review.note_ids {
                if let Some(t) = review.last_review_at {
                    reviewed
                        .entry(id)
                        .and_modify(|v| *v = (*v).max(t))
                        .or_insert(t);
                }
                if let Some(t) = review.next_review_at {
                    due.entry(id).and_modify(|v| *v = (*v).min(t)).or_insert(t);
                }
            }
        }
        let folder_parents: HashMap<_, _> = snapshot
            .folders
            .iter()
            .map(|f| (f.id.as_str(), f.parent_id.as_deref()))
            .collect();
        for folder in &snapshot.folders {
            let mut ancestors = Vec::new();
            let mut parent = folder.parent_id.as_deref();
            let mut seen = HashSet::from([folder.id.as_str()]);
            while let Some(id) = parent.filter(|id| seen.insert(*id)) {
                ancestors.push(id.to_owned());
                parent = folder_parents.get(id).copied().flatten();
            }
            result.relations.insert(
                folder.id.clone(),
                FolderRelation {
                    parent: folder.parent_id.clone(),
                    ancestors,
                    subtree_height: 1,
                },
            );
        }
        for folder in &snapshot.folders {
            let ancestors = result.relations[&folder.id].ancestors.clone();
            for (distance, ancestor) in ancestors.iter().enumerate() {
                if let Some(relation) = result.relations.get_mut(ancestor) {
                    relation.subtree_height = relation.subtree_height.max(distance + 2);
                }
            }
        }
        let stamp = |t: Option<&i64>, empty: &str| {
            t.map(|t| crate::presentation::stamp(*t))
                .unwrap_or_else(|| empty.into())
        };
        for (position, note) in snapshot.notes.iter().enumerate() {
            let version = latest.get(note.id.as_str());
            let content = version
                .map(|v| v.content.to_lowercase())
                .unwrap_or_default();
            result.content_bodies_indexed += usize::from(version.is_some());
            let mut ancestors = Vec::new();
            let mut parent = note.folder_id.as_deref();
            let mut seen = HashSet::new();
            while let Some(id) = parent.filter(|id| seen.insert(*id)) {
                ancestors.push(id.to_owned());
                parent = folder_parents.get(id).copied().flatten();
            }
            let status = StudyStatus::from_history(
                version.map(|v| v.hash.as_str()),
                recorded.get(note.id.as_str()).copied(),
                baseline.get(note.id.as_str()).copied(),
            );
            result.notes.insert(
                note.id.clone(),
                NoteMetadata {
                    position,
                    updated_stamp: crate::presentation::stamp(note.updated_at),
                    ancestors: ancestors.clone(),
                    title_key: note.title.to_lowercase(),
                    version_count: counts.get(note.id.as_str()).copied().unwrap_or(0),
                    status,
                    tip: format!(
                        "{}\n{}\n学习时间：{}\n上次复习：{}\n下次复习：{}",
                        source_label(note),
                        status.label(),
                        stamp(learned.get(note.id.as_str()), "尚无学习记录"),
                        stamp(reviewed.get(note.id.as_str()), "尚未复习"),
                        stamp(due.get(note.id.as_str()), "暂无待复习计划")
                    ),
                    search: format!(
                        "{}\n{}\n{}",
                        note.title.to_lowercase(),
                        note.path.to_lowercase(),
                        content
                    ),
                },
            );
            if let Some(id) = &note.folder_id {
                result.direct_folders.insert(id.clone());
            }
            for id in ancestors {
                result
                    .folders
                    .entry(id)
                    .and_modify(|t| *t = (*t).max(note.updated_at))
                    .or_insert(note.updated_at);
            }
        }
        for (slot, sort) in [
            LibrarySort::NameAscending,
            LibrarySort::NameDescending,
            LibrarySort::Newest,
            LibrarySort::Oldest,
        ]
        .into_iter()
        .enumerate()
        {
            let mut order: Vec<_> = (0..snapshot.notes.len()).collect();
            order.sort_by(|&a, &b| {
                let a = &snapshot.notes[a];
                let b = &snapshot.notes[b];
                let primary = match sort {
                    LibrarySort::NameAscending => result.notes[&a.id]
                        .title_key
                        .cmp(&result.notes[&b.id].title_key),
                    LibrarySort::NameDescending => result.notes[&b.id]
                        .title_key
                        .cmp(&result.notes[&a.id].title_key),
                    LibrarySort::Newest => b.updated_at.cmp(&a.updated_at),
                    LibrarySort::Oldest => a.updated_at.cmp(&b.updated_at),
                };
                primary.then_with(|| a.id.cmp(&b.id))
            });
            result.order[slot] = order;
        }
        result
    }
}
