//! Lightweight draft list/search data maintained at catalog and query boundaries.
//! Content is borrowed only while a changed draft/query is searched; no full-size
//! lowercase copies are retained by this model.
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

pub struct DraftRow<'a> {
    pub id: &'a str,
    pub revision: u64,
    pub updated_at: i64,
    pub active: bool,
    pub title: &'a str,
    pub content: &'a str,
    pub note_id: Option<&'a str>,
    pub folder_id: Option<&'a str>,
}

#[derive(PartialEq, Eq)]
struct Key {
    revision: u64,
    updated_at: i64,
    active: bool,
    title: String,
    note_id: Option<String>,
    folder_id: Option<String>,
}
struct Entry {
    key: Key,
    matches: bool,
}

#[derive(Default)]
pub struct DraftLibraryIndex {
    query: String,
    query_changed: bool,
    entries: HashMap<String, Entry>,
    ids: Arc<Vec<String>>,
    active_count: usize,
    /// Work instrumentation counts actual body bytes considered, not cache hits.
    pub body_bytes_searched: usize,
}

impl DraftLibraryIndex {
    pub fn ids(&self) -> &[String] {
        &self.ids
    }
    pub fn shared_ids(&self) -> Arc<Vec<String>> {
        self.ids.clone()
    }
    pub fn active_count(&self) -> usize {
        self.active_count
    }
    pub fn set_query(&mut self, query: &str) -> bool {
        let query = query.to_lowercase();
        if self.query != query {
            self.query = query;
            self.query_changed = true;
            true
        } else {
            false
        }
    }
    /// Call when the catalog changes or after set_query. DraftRecord::revision
    /// is the authority for content changes; metadata is also compared exactly.
    pub fn refresh<'a>(&mut self, rows: impl IntoIterator<Item = DraftRow<'a>>) {
        let mut present = HashSet::new();
        let mut changed = self.query_changed;
        for row in rows {
            present.insert(row.id.to_owned());
            let key = Key {
                revision: row.revision,
                updated_at: row.updated_at,
                active: row.active,
                title: row.title.to_owned(),
                note_id: row.note_id.map(str::to_owned),
                folder_id: row.folder_id.map(str::to_owned),
            };
            if !self.query_changed
                && self
                    .entries
                    .get(row.id)
                    .is_some_and(|entry| entry.key == key)
            {
                continue;
            }
            let matches = row.active
                && (self.query.is_empty() || row.title.to_lowercase().contains(&self.query) || {
                    self.body_bytes_searched += row.content.len();
                    row.content.to_lowercase().contains(&self.query)
                });
            self.entries
                .insert(row.id.to_owned(), Entry { key, matches });
            changed = true;
        }
        self.entries.retain(|id, _| {
            let retained = present.contains(id);
            changed |= !retained;
            retained
        });
        self.query_changed = false;
        if changed {
            self.active_count = self
                .entries
                .values()
                .filter(|entry| entry.key.active)
                .count();
            let mut ids: Vec<_> = self
                .entries
                .iter()
                .filter(|(_, entry)| entry.matches)
                .map(|(id, _)| id.clone())
                .collect();
            ids.sort_by(|a, b| {
                self.entries[b]
                    .key
                    .updated_at
                    .cmp(&self.entries[a].key.updated_at)
                    .then_with(|| a.cmp(b))
            });
            self.ids = Arc::new(ids);
        }
    }
}
