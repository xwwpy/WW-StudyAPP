#[path = "storage_purge_remote.rs"]
pub mod remote;
use super::{guard_directories, indexed_images, owned_path, plain_handle, root, visit_tree};
use crate::drafts::{DraftRecord, DraftStatus, DraftStore, NoteDraft};
use anyhow::{Context, Result, ensure};
use pulldown_cmark::{Event, Options, Parser, Tag};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use studyapp_core::{Engine, Snapshot};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Stamp {
    identity: (u64, u128),
    hash: [u8; 32],
    pub bytes: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub library: String,
    pub note_id: String,
    pub revision: u64,
    pub files: BTreeMap<PathBuf, Stamp>,
    backups: BTreeMap<PathBuf, Stamp>,
    backup_revisions: BTreeMap<PathBuf, [u8; 32]>,
    pub keys: BTreeSet<String>,
    pub shared: usize,
    pub versions: usize,
    pub drafts: usize,
    pub unresolved: BTreeSet<PathBuf>,
    evidence: [u8; 32],
    live_notes: BTreeMap<PathBuf, Stamp>,
}
impl Plan {
    pub fn paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.files.keys().chain(self.backups.keys())
    }
    pub fn bytes(&self) -> u64 {
        self.files
            .values()
            .fold(0u64, |b, s| b.saturating_add(s.bytes))
    }
    pub fn backup_count(&self) -> usize {
        self.backups.len()
    }
    fn guard_live_references(&self) -> Result<Vec<fs::File>> {
        let mut handles = Vec::new();
        if self.keys.is_empty() {
            return Ok(handles);
        }
        for (path, expected) in &self.live_notes {
            let mut file = plain_handle(path, false, true)?;
            let mut digest = Sha256::new();
            let bytes = std::io::copy(&mut file, &mut digest)?;
            ensure!(
                crate::app_usage::identity(&file)? == expected.identity
                    && bytes == expected.bytes
                    && <[u8; 32]>::from(digest.finalize()) == expected.hash,
                "跟踪笔记 {} 已改变，请重新预览",
                path.display()
            );
            // Deny replacement and writes for this batch on Windows. Other
            // platforms re-observe the sources at each destructive batch.
            if cfg!(windows) {
                handles.push(file);
            }
        }
        Ok(handles)
    }
}

fn stamp(path: &Path) -> Result<Stamp> {
    let _guard = guard_directories(path.parent().context("文件目录无效")?, false)?;
    let mut f = plain_handle(path, false, false)?;
    let identity = crate::app_usage::identity(&f)?;
    let mut hash = Sha256::new();
    let bytes = std::io::copy(&mut f, &mut hash)?;
    ensure!(bytes == f.metadata()?.len(), "文件已变化，请重新确认");
    Ok(Stamp {
        identity,
        hash: hash.finalize().into(),
        bytes,
    })
}
fn entries(dir: &Path) -> Result<Vec<PathBuf>> {
    if !dir.try_exists()? {
        return Ok(vec![]);
    }
    let _guard = guard_directories(dir, false)?;
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        ensure!(out.len() < 100_000, "目录条目过多，无法完整确认删除范围");
        out.push(entry?.path())
    }
    out.sort();
    Ok(out)
}
pub(crate) fn keys_in(text: &str) -> BTreeSet<String> {
    let mut keys: BTreeSet<_> = Parser::new_ext(text, Options::all())
        .filter_map(|e| match e {
            Event::Start(Tag::Image { dest_url, .. })
            | Event::Start(Tag::Link { dest_url, .. }) => key_from(&dest_url),
            _ => None,
        })
        .collect();
    // A draft may end in the middle of a Markdown image expression.
    for raw in text.split("studyapp-media:").skip(1) {
        let tail = raw
            .split(|c: char| !(c.is_alphanumeric() || matches!(c, '/' | '_' | '-' | '.')))
            .next()
            .unwrap_or("");
        if let Some(key) = key_from(&format!("studyapp-media:{}", tail.trim_end_matches('.'))) {
            keys.insert(key);
        }
    }
    keys
}
fn key_from(dest: &str) -> Option<String> {
    let key = format!("assets/{}", dest.strip_prefix("studyapp-media:")?);
    studyapp_sync::validate_asset_key(&key).ok()?;
    Some(key)
}
// Used exclusively to protect possible aliases. Ownership always requires the
// original, validated key and independently verified paths.
fn folded(text: &str) -> String {
    let normalizer = icu_normalizer::ComposingNormalizer::new_nfkc();
    let normalized = normalizer.normalize(text);
    let case_folded = unicase::UniCase::unicode(normalized.as_ref()).to_folded_case();
    normalizer.normalize(&case_folded).into_owned()
}
fn protected_keys(text: &str) -> BTreeSet<String> {
    if !text.contains("studyapp-media:") {
        return BTreeSet::new();
    }
    keys_in(&folded(text))
}
#[derive(Default)]
struct Refs {
    mine: BTreeSet<String>,
    shared: BTreeSet<String>,
    hashes: BTreeSet<String>,
    other_hashes: BTreeSet<String>,
    other_paths: BTreeSet<PathBuf>,
    protected_keys: BTreeSet<String>,
}
impl Refs {
    fn snapshot(&mut self, s: &Snapshot, id: &str) {
        for v in s.versions.iter() {
            let mine = v.note_id == id;
            self.text(mine, &v.content);
            if mine {
                self.hashes.insert(v.hash.clone());
            } else {
                self.other_hashes.insert(v.hash.clone());
                let source = s
                    .notes
                    .iter()
                    .find(|n| n.id == v.note_id)
                    .map(|n| Path::new(&n.path));
                self.local_paths(&v.content, source);
            }
        }
        for session in &s.sessions {
            self.text(false, &session.summary);
            self.local_paths(&session.summary, None);
        }
    }
    fn local_paths(&mut self, text: &str, source: Option<&Path>) {
        for e in Parser::new_ext(text, Options::all()) {
            if let Event::Start(Tag::Image { dest_url, .. }) = e {
                let Ok(raw) = percent_encoding::percent_decode_str(&dest_url).decode_utf8() else {
                    continue;
                };
                let local_extended = matches!(Path::new(raw.as_ref()).components().next(),
                    Some(std::path::Component::Prefix(prefix)) if matches!(prefix.kind(),std::path::Prefix::VerbatimDisk(_)));
                if raw.starts_with("//")
                    || (raw.starts_with(r"\\") && !local_extended)
                    || raw.contains("://")
                    || raw.starts_with("studyapp-media:")
                {
                    continue;
                }
                let path = Path::new(raw.as_ref());
                let path = if path.is_absolute() {
                    path.to_owned()
                } else if let Some(dir) = source.and_then(Path::parent) {
                    dir.join(path)
                } else {
                    continue;
                };
                self.other_paths.insert(path);
            }
        }
    }
    fn text(&mut self, mine: bool, text: &str) {
        if !mine {
            self.protected_keys.extend(protected_keys(text));
        }
        if mine {
            &mut self.mine
        } else {
            &mut self.shared
        }
        .extend(keys_in(text))
    }
}

fn read_tracked(path: &Path) -> Result<(String, Stamp)> {
    let _dirs = guard_directories(path.parent().context("笔记目录无效")?, false)?;
    let mut file = plain_handle(path, false, true)?;
    let mut body = String::new();
    file.read_to_string(&mut body)?;
    let observed = stamp(path)?;
    ensure!(
        observed.identity == crate::app_usage::identity(&file)?
            && observed.hash == <[u8; 32]>::from(Sha256::digest(body.as_bytes())),
        "跟踪文件已改变"
    );
    Ok((body, observed))
}

fn image_identity(path: &Path) -> Result<Option<(u64, u128)>> {
    let path = match path.canonicalize() {
        Ok(path) => path,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    Ok(Some(crate::app_usage::identity(&plain_handle(
        &path, false, false,
    )?)?))
}
fn draft_files(s: &Snapshot, plan: &mut Plan, refs: &mut Refs) -> Result<BTreeSet<String>> {
    let mut groups = BTreeSet::from([plan.note_id.clone()]);
    let legacy = DraftStore::path(&s.data_path);
    for path in entries(&DraftStore::directory(&s.data_path))?
        .into_iter()
        .chain(std::iter::once(legacy.clone()))
    {
        if !path.try_exists()? {
            continue;
        }
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        if path != legacy && !matches!(ext, "json" | "next") {
            continue;
        }
        let _guards = guard_directories(path.parent().context("草稿目录无效")?, false)?;
        let mut file = plain_handle(&path, false, true)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let saved = stamp(&path)?;
        ensure!(
            saved.identity == crate::app_usage::identity(&file)?
                && saved.hash == <[u8; 32]>::from(Sha256::digest(&bytes)),
            "草稿已变化，请重新预览"
        );
        let (draft, owned, bindings, source) = if path == legacy {
            let Some(draft) = serde_json::from_slice::<Option<NoteDraft>>(&bytes)? else {
                continue;
            };
            let mine = draft.note_id.as_deref() == Some(plan.note_id.as_str());
            (draft, mine, BTreeMap::new(), None)
        } else {
            let r: DraftRecord =
                serde_json::from_slice(&bytes).context("草稿无法核对，保留原文件")?;
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            ensure!(
                r.schema == 2
                    && r.revision > 0
                    && uuid::Uuid::parse_str(&r.id).is_ok_and(|u| u.to_string() == r.id)
                    && (name == format!("{}.json", r.id)
                        || uuid_named(name, &format!("{}.", r.id), ".next")),
                "草稿身份不一致"
            );
            let mine = r.draft.note_id.as_ref() == Some(&plan.note_id)
                || r.save_target.as_ref() == Some(&plan.note_id)
                || matches!(&r.status,DraftStatus::Saved{note_id,..} if note_id==&plan.note_id);
            (r.draft, mine, r.media_bindings, r.source_path)
        };
        plan.evidence = Sha256::digest([plan.evidence, saved.hash].concat()).into();
        if !owned {
            refs.other_hashes
                .insert(format!("{:x}", Sha256::digest(draft.content.as_bytes())));
            let source = source.as_deref().or_else(|| {
                s.notes
                    .iter()
                    .find(|n| Some(&n.id) == draft.note_id.as_ref())
                    .map(|n| Path::new(&n.path))
            });
            refs.local_paths(&draft.content, source);
        }
        refs.text(owned, &draft.content);
        let set = if owned {
            &mut refs.mine
        } else {
            &mut refs.shared
        };
        set.extend(bindings.values().filter_map(|v| key_from(v)));
        if !owned {
            for binding in bindings.values() {
                refs.protected_keys.extend(protected_keys(binding));
            }
        }
        if owned {
            groups.insert(draft.asset_group);
            plan.files.insert(path.clone(), saved);
            plan.drafts += 1;
        }
    }
    Ok(groups)
}
fn uuid_named(name: &str, start: &str, end: &str) -> bool {
    name.strip_prefix(start)
        .and_then(|n| n.strip_suffix(end))
        .is_some_and(|n| uuid::Uuid::parse_str(n).is_ok_and(|u| u.to_string() == n))
}
fn retain_aliases(plan: &mut Plan, parent: &Path) -> Result<()> {
    let originals: BTreeSet<_> = plan
        .files
        .iter()
        .filter(|(p, _)| p.parent() == Some(parent))
        .map(|(_, s)| s.identity)
        .collect();
    for sibling in entries(parent)? {
        let name = sibling.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if uuid_named(name, ".studyapp-image-", ".pending") {
            let s = stamp(&sibling)?;
            if originals.contains(&s.identity) {
                plan.files.insert(sibling, s);
            }
        }
    }
    Ok(())
}

// A journal never authorizes a deletion. Each retry rebuilds ownership and
// reference evidence. Old paths which can no longer be attributed are reported.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Inventory {
    files: BTreeMap<PathBuf, Stamp>,
    backups: BTreeMap<PathBuf, Stamp>,
}
fn inventory_prefix(id: &str) -> String {
    format!("scope-{id}-")
}
fn prior_inventory(dir: &Path, id: &str) -> Result<Inventory> {
    let mut all = Inventory::default();
    for path in entries(&dir.join(".purged-notes"))? {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !uuid_named(name, &inventory_prefix(id), ".json") {
            continue;
        }
        let file = plain_handle(&path, false, true)?;
        ensure!(file.metadata()?.len() <= 64 * 1024 * 1024, "删除记录过大");
        let saved: Inventory = serde_json::from_reader(file)?;
        all.files.extend(saved.files);
        all.backups.extend(saved.backups);
    }
    Ok(all)
}
fn auto_backup(path: &Path) -> bool {
    let Some(rest) = path
        .file_name()
        .and_then(|p| p.to_str())
        .and_then(|n| n.strip_prefix("studyapp-before-restore-"))
        .and_then(|n| n.strip_suffix(".sqlite"))
    else {
        return false;
    };
    let Some((time, uuid)) = rest.split_once('-') else {
        return false;
    };
    time.parse::<u64>().is_ok() && uuid::Uuid::parse_str(uuid).is_ok_and(|u| u.to_string() == uuid)
}

pub fn prepare(s: &Snapshot, id: &str) -> Result<Plan> {
    let note = s.notes.iter().find(|n| n.id == id).context("找不到笔记")?;
    let path = owned_path(s, note)?;
    let dir = root(s)?;
    let prior = prior_inventory(dir, id)?;
    let image_root = s
        .settings
        .images
        .root
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| dir.join("images"));
    let approved = [
        dir.canonicalize()?,
        image_root
            .canonicalize()
            .unwrap_or_else(|_| image_root.clone()),
    ];
    // A crash can hide an approved note, draft, index, or retained writer link.
    // Restore matching claims before recomputing references or file ownership.
    let mut recovered = BTreeSet::new();
    for p in prior.files.keys() {
        if !p
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
            && approved.iter().any(|root| p.starts_with(root))
            && let Some(parent) = p.parent()
            && recovered.insert(parent.to_owned())
        {
            crate::app_migration::recover_removal_parent(parent)?;
        }
    }
    let mut plan = Plan {
        library: s.data_path.clone(),
        note_id: id.into(),
        revision: note.revision,
        files: BTreeMap::new(),
        backups: BTreeMap::new(),
        backup_revisions: BTreeMap::new(),
        keys: BTreeSet::new(),
        shared: 0,
        versions: s.versions.iter().filter(|v| v.note_id == id).count(),
        drafts: 0,
        live_notes: BTreeMap::new(),
        unresolved: BTreeSet::new(),
        evidence: Sha256::digest(serde_json::to_vec(&(
            &s.settings,
            &s.notes,
            &s.versions
                .iter()
                .map(|v| (&v.id, &v.hash))
                .collect::<Vec<_>>(),
        ))?)
        .into(),
    };
    let mut refs = Refs::default();
    refs.snapshot(s, id);
    let mut unreadable = None;
    for other in s.notes.iter().filter(|n| n.id != id && !n.detached) {
        let path = Path::new(&other.path);
        match read_tracked(path) {
            Ok((body, stamp)) => {
                refs.text(false, &body);
                refs.local_paths(&body, Some(path));
                refs.other_hashes
                    .insert(format!("{:x}", Sha256::digest(body.as_bytes())));
                plan.live_notes.insert(path.to_owned(), stamp);
            }
            Err(e) => {
                unreadable = Some(e.context(format!(
                    "无法核对跟踪笔记 {} 的当前内容，不能安全删除图片",
                    path.display()
                )));
            }
        }
    }
    let groups = draft_files(s, &mut plan, &mut refs)?;
    for p in entries(dir)? {
        if !auto_backup(&p) || p == Path::new(&s.data_path) {
            continue;
        }
        let _lease = crate::maintenance_runtime::no_delete_file_handle(&p).with_context(|| {
            format!(
                "自动备份正被占用、存在额外链接或无法验证，请先核对 {}",
                p.display()
            )
        })?;
        let saved = stamp(&p)?;
        if let Some(backup) = studyapp_core::inspect_purge_backup(&p)? {
            // SQLite's main file may stay unchanged while WAL records change.
            let revision = studyapp_core::purge_backup_revision(&backup)?;
            plan.evidence = Sha256::digest([plan.evidence, revision].concat()).into();
            let source_matches = backup
                .notes
                .iter()
                .any(|n| n.id == id && n.managed && super::same_path(Path::new(&n.path), &path));
            let resume = prior
                .backups
                .get(&p)
                .is_some_and(|old| old.identity == saved.identity)
                && !backup.notes.iter().any(|n| n.id == id);
            refs.snapshot(&backup, if source_matches { id } else { "" });
            if source_matches || resume {
                plan.backup_revisions.insert(p.clone(), revision);
                plan.backups.insert(p, saved);
            } else if backup.notes.iter().any(|n| n.id == id) {
                plan.unresolved.insert(p);
            }
        }
    }
    if path.try_exists()? {
        plan.files.insert(path.clone(), stamp(&path)?);
    }
    let originals = format!("-{id}.md");
    let mut pending = vec![];
    for p in entries(path.parent().context("托管目录无效")?)? {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if uuid_named(name, ".original-", &originals) {
            plan.files.insert(p.clone(), stamp(&p)?);
        } else if uuid_named(name, ".pending-", ".md") || uuid_named(name, ".original-", ".md") {
            pending.push((p.clone(), stamp(&p)?));
        }
    }
    let identities: BTreeSet<_> = plan.files.values().map(|s| s.identity).collect();
    for (p, s) in pending {
        let hash = format!("{:x}", sha2::digest::Output::<Sha256>::from(s.hash));
        if identities.contains(&s.identity)
            || (unreadable.is_none()
                && refs.hashes.contains(&hash)
                && !refs.other_hashes.contains(&hash))
        {
            plan.files.insert(p, s);
        } else if refs.hashes.contains(&hash) || !refs.other_hashes.contains(&hash) {
            plan.unresolved.insert(p);
        }
    }

    let index = dir.join("image-index");
    let mut assets: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    if index.try_exists()? {
        visit_tree(
            &index,
            Path::new(""),
            true,
            &|_| false,
            &mut 0,
            &mut |p, relative, f| {
                let location = Path::new("image-index").join(relative);
                let paths = indexed_images(s, &location, f)?;
                if !paths.is_empty() {
                    let tail = relative
                        .to_str()
                        .context("图片索引名无效")?
                        .trim_end_matches(".json")
                        .replace('\\', "/");
                    let key = format!("assets/{tail}");
                    if refs.protected_keys.contains(&folded(&key)) {
                        refs.other_paths.extend(paths.iter().cloned());
                    }
                    if groups.contains(key.split('/').nth(1).unwrap_or(""))
                        || refs.mine.contains(&key)
                    {
                        let allowed = assets.entry(key).or_default();
                        for candidate in paths {
                            // A legacy library copy can still point to the image
                            // in its retained source. A matching hidden link in
                            // that library proves its ownership, not ours.
                            let current = image_root.join(&tail);
                            if candidate != current
                                && !candidate
                                    .canonicalize()
                                    .ok()
                                    .zip(current.canonicalize().ok())
                                    .is_some_and(|(a, b)| a == b)
                            {
                                plan.unresolved.insert(candidate);
                            } else {
                                allowed.push(candidate);
                            }
                        }
                        allowed.push(p.to_owned());
                    }
                }
                Ok(())
            },
        )?;
    }
    for key in &refs.mine {
        assets
            .entry(key.clone())
            .or_default()
            .push(image_root.join(key.trim_start_matches("assets/")));
    }
    // Publishing can be interrupted after the image file is created but before
    // its index is written. Only application-generated names in an owned group
    // qualify here, never arbitrary files in a configured external folder.
    for group in &groups {
        if !uuid::Uuid::parse_str(group).is_ok_and(|u| u.to_string() == *group) {
            continue;
        }
        for p in entries(&image_root.join(group))? {
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let key = format!("assets/{group}/{name}");
            if studyapp_sync::validate_asset_key(&key).is_ok()
                && p.file_stem()
                    .and_then(|n| n.to_str())
                    .and_then(|n| n.rsplit('-').next())
                    .is_some_and(|suffix| {
                        suffix.len() == 32 && suffix.bytes().all(|b| b.is_ascii_hexdigit())
                    })
            {
                assets.entry(key).or_default().push(p);
            }
        }
    }
    let mut parents = BTreeSet::new();
    if !assets.is_empty()
        && let Some(error) = unreadable
    {
        return Err(error);
    }
    for key in &refs.shared {
        refs.other_paths
            .insert(image_root.join(key.trim_start_matches("assets/")));
    }
    let mut protected_ids = BTreeSet::new();
    for p in &refs.other_paths {
        if let Some(identity) = image_identity(p)? {
            protected_ids.insert(identity);
        }
    }
    for (key, paths) in assets {
        let mut shared = refs.protected_keys.contains(&folded(&key));
        for path in &paths {
            if image_identity(path)?.is_some_and(|id| protected_ids.contains(&id)) {
                shared = true;
                break;
            }
        }
        if shared {
            plan.shared += 1;
            continue;
        }
        plan.keys.insert(key);
        for p in paths {
            if p.try_exists()? {
                let parent = p.parent().context("图片路径无效")?;
                let canonical = p.canonicalize()?;
                if !dir
                    .join("images")
                    .canonicalize()
                    .is_ok_and(|r| canonical.starts_with(r))
                    && !index.canonicalize().is_ok_and(|r| canonical.starts_with(r))
                {
                    let asset_base = parent.parent().context("图片目录无效")?;
                    let current_library = dir.canonicalize()?;
                    let mut retained_library = false;
                    for ancestor in asset_base.canonicalize()?.ancestors() {
                        if ancestor == current_library {
                            break;
                        }
                        if ancestor.join("studyapp.sqlite3").try_exists()?
                            || ancestor.join("image-index").try_exists()?
                        {
                            retained_library = true;
                            break;
                        }
                    }
                    if retained_library {
                        plan.unresolved.insert(p);
                        continue;
                    }
                    let id = stamp(&p)?.identity;
                    let mut verified = false;
                    for alias in entries(parent)? {
                        if alias
                            .file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| uuid_named(n, ".studyapp-image-", ".pending"))
                            && stamp(&alias)?.identity == id
                        {
                            verified = true;
                            break;
                        }
                    }
                    if !verified {
                        plan.unresolved.insert(p);
                        continue;
                    }
                }
                parents.insert(parent.to_owned());
                plan.files.insert(p.clone(), stamp(&p)?);
            }
        }
    }
    for p in parents {
        retain_aliases(&mut plan, &p)?;
    }
    // The index may name the same directory entry with and without the Windows
    // extended path prefix. Preserve distinct hardlink names, not path aliases.
    plan.files = plan
        .files
        .into_iter()
        .map(|(p, s)| Ok((fs::canonicalize(p)?, s)))
        .collect::<Result<_>>()?;
    for group in &groups {
        if !uuid::Uuid::parse_str(group).is_ok_and(|u| u.to_string() == *group) {
            continue;
        }
        for base in [&image_root, &index] {
            let siblings = entries(&base.join(group))?;
            let mut published = BTreeSet::new();
            for p in &siblings {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !name.starts_with('.') && p.is_file() {
                    published.insert(stamp(p)?.identity);
                }
            }
            for p in siblings {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if (uuid_named(name, ".studyapp-image-", ".pending")
                    && !published.contains(&stamp(&p)?.identity))
                    || uuid_named(name, ".studyapp-image-", ".invalid")
                {
                    plan.unresolved.insert(p);
                }
            }
        }
    }
    let approved_roots = [
        dir.canonicalize()?,
        image_root.canonicalize().unwrap_or(image_root),
    ];
    for p in prior.files.keys().chain(prior.backups.keys()) {
        if plan.files.contains_key(p) || plan.backups.contains_key(p) {
            continue;
        }
        // Untrusted journal data cannot make us probe external paths. Historical
        // external locations need independent confirmation on every attempt.
        let bounded = !p
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
            && (approved_roots.iter().any(|base| p.starts_with(base))
                || (p.parent() == Some(dir) && auto_backup(p)));
        if !bounded || p.try_exists()? {
            plan.unresolved.insert(p.clone());
        }
    }
    Ok(plan)
}
pub fn suppressed(s: &Snapshot) -> Result<BTreeSet<String>> {
    let mut ids = BTreeSet::new();
    for p in entries(&root(s)?.join(".purged-notes"))? {
        if let Some(name) = p.file_name().and_then(|n| n.to_str())
            && uuid::Uuid::parse_str(name).is_ok_and(|u| u.to_string() == name)
        {
            plain_handle(&p, false, false)?;
            ids.insert(name.into());
        }
    }
    Ok(ids)
}
pub fn pending_cleanup(s: &Snapshot) -> Result<Vec<String>> {
    let mut pending = vec![];
    for id in suppressed(s)? {
        if s.notes.iter().any(|n| n.id == id) {
            continue;
        }
        let marker = root(s)?.join(".purged-notes").join(&id);
        if marker.with_extension("done").try_exists()? {
            continue;
        }
        pending.push(id);
    }
    Ok(pending)
}
pub fn retry_database_cleanup(s: &Snapshot) -> Result<()> {
    for id in pending_cleanup(s)? {
        let marker = root(s)?.join(".purged-notes").join(&id);
        studyapp_core::finish_purge_cleanup(Path::new(&s.data_path), &id)?;
        finish_marker(&marker)?;
    }
    Ok(())
}
fn finish_marker(marker: &Path) -> Result<()> {
    let _guards = guard_directories(marker.parent().context("删除标记目录无效")?, false)?;
    let done = marker.with_extension("done");
    if !done.try_exists()? {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(done)?
            .sync_all()?;
    }
    Ok(())
}
pub fn execute(engine: &mut Engine, plan: &Plan, now: i64) -> Result<()> {
    let fresh = prepare(&engine.snapshot(), &plan.note_id)?;
    ensure!(&fresh == plan, "删除清单已变化，请重新确认");
    engine.verify_purge_ready()?;
    let marker = Path::new(&plan.library)
        .parent()
        .context("资料库位置无效")?
        .join(".purged-notes")
        .join(&plan.note_id);
    let parent = marker.parent().unwrap();
    let _guards = guard_directories(parent, true)?;
    let prefix = inventory_prefix(&plan.note_id);
    let uuid = uuid::Uuid::new_v4();
    let staging = parent.join(format!("{prefix}{uuid}.next"));
    let mut journal = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&staging)?;
    serde_json::to_writer(
        &mut journal,
        &Inventory {
            files: plan.files.clone(),
            backups: plan.backups.clone(),
        },
    )?;
    journal.sync_all()?;
    drop(journal);
    fs::rename(staging, parent.join(format!("{prefix}{uuid}.json")))?;
    let done = marker.with_extension("done");
    if done.try_exists()? {
        plain_handle(&done, false, true)?;
        fs::remove_file(done)?;
    }
    if !marker.try_exists()? {
        let mut out = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&marker)?;
        out.write_all(b"1\n")?;
        out.sync_all()?;
    }
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    for (p, s) in &plan.backups {
        let _parent = guard_directories(p.parent().context("备份目录无效")?, false)?;
        let _lease = crate::maintenance_runtime::no_delete_file_handle(p)?;
        ensure!(stamp(p)? == *s, "备份已变化，请重新确认");
        studyapp_core::scrub_confirmed_backup(p, &plan.note_id, plan.backup_revisions[p])?;
    }
    let _live_refs = plan.guard_live_references()?;
    // Remove data while its ownership evidence still exists for a later retry.
    let mut files: Vec<_> = plan.files.iter().collect();
    let db_root = root(&engine.snapshot())?.canonicalize()?;
    files.sort_by_key(|(p, _)| {
        if p.starts_with(db_root.join("notes")) {
            4
        } else if p.starts_with(db_root.join("drafts"))
            || p.file_name().is_some_and(|n| n == "note-draft.json")
        {
            3
        } else if p.starts_with(db_root.join("image-index")) {
            2
        } else if p
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with(".studyapp-image-"))
        {
            1
        } else {
            0
        }
    });
    for (p, s) in files {
        crate::app_migration::unlink_exact(p, s.identity, &s.hash)
            .with_context(|| format!("未能清除 {}，可重新预览后重试", p.display()))?;
    }
    engine.purge_managed_note(&plan.note_id, plan.revision, now)?;
    finish_marker(&marker)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use studyapp_core::Command;
    #[cfg(unix)]
    #[test]
    fn fresh_purge_restores_and_removes_previously_claimed_note() {
        let root = tempfile::tempdir().unwrap().keep();
        let id = uuid::Uuid::new_v4().to_string();
        let mut engine = Engine::open(&root.join("studyapp.sqlite3"), 10).unwrap();
        engine
            .apply(
                Command::CreateNoteWithId {
                    note_id: id.clone(),
                    title: "original".into(),
                    content: "bytes".into(),
                    folder_id: None,
                },
                11,
            )
            .unwrap();
        let plan = prepare(&engine.snapshot(), &id).unwrap();
        let markers = root.join(".purged-notes");
        fs::create_dir_all(&markers).unwrap();
        let inventory = Inventory {
            files: plan.files.clone(),
            backups: plan.backups.clone(),
        };
        fs::write(
            markers.join(format!("scope-{id}-{}.json", uuid::Uuid::new_v4())),
            serde_json::to_vec(&inventory).unwrap(),
        )
        .unwrap();
        let note = root.join("notes").join(format!("{id}.md"));
        let claim = crate::app_migration::simulate_interrupted_claim(&note);
        let retried = prepare(&engine.snapshot(), &id).unwrap();
        assert!(note.exists() && !claim.exists());
        execute(&mut engine, &retried, 12).unwrap();
        assert!(!note.exists() && !claim.exists());
    }
    #[test]
    fn live_external_markdown_invalidates_preview_and_protects_an_image_alias() {
        let dir = tempfile::tempdir().unwrap().keep();
        let mut engine = Engine::open(&dir.join("learn.sqlite3"), 10).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        engine
            .apply(
                Command::CreateNoteWithId {
                    note_id: id.clone(),
                    title: "target".into(),
                    content: format!("![target](studyapp-media:{id}/photo.gif)"),
                    folder_id: None,
                },
                11,
            )
            .unwrap();
        let source = dir.join("external.md");
        fs::write(&source, "unrelated").unwrap();
        engine
            .apply(Command::Import(vec![source.clone()]), 12)
            .unwrap();
        let group = dir.join("images").join(&id);
        fs::create_dir_all(&group).unwrap();
        let target = group.join("photo.gif");
        fs::write(&target, "image").unwrap();
        fs::hard_link(&target, group.join("other.gif")).unwrap();
        let snapshot = engine.snapshot();
        let original = prepare(&snapshot, &id).unwrap();
        assert!(original.files.contains_key(&target.canonicalize().unwrap()));
        // Disk changes before the file watcher has published a new version.
        fs::write(&source, format!("![other](images/{id}/other.gif)")).unwrap();
        assert!(execute(&mut engine, &original, 13).is_err());
        let fresh = prepare(&snapshot, &id).unwrap();
        assert_eq!(fresh.shared, 1);
        assert!(!fresh.files.contains_key(&target.canonicalize().unwrap()));
        fs::write(&source, format!("![other](studyapp-media:{id}/other.gif)")).unwrap();
        assert_eq!(prepare(&snapshot, &id).unwrap().shared, 1);
        fs::write(&source, [0xff]).unwrap();
        assert!(
            prepare(&snapshot, &id)
                .unwrap_err()
                .to_string()
                .contains("无法核对跟踪笔记")
        );
    }

    #[test]
    fn shared_unicode_names_preserve_composition_and_case_aliases() {
        let id = uuid::Uuid::new_v4().to_string();
        let protected = protected_keys(&format!(
            "![shared](studyapp-media:{id}/CAFE\u{301}-Straße.GIF)"
        ));
        assert!(protected.contains(&folded(&format!("assets/{id}/café-strasse.gif"))));
    }
    #[test]
    fn shared_image_survives_and_changed_file_invalidates_confirmation() {
        let root = tempfile::tempdir().unwrap().keep();
        let db = root.join("studyapp.sqlite3");
        let mut engine = Engine::open(&db, 10).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let body = format!(
            "![one](studyapp-media:{id}/one.gif) ![shared](studyapp-media:{id}/shared.gif) ![relative](studyapp-media:{id}/relative.gif)"
        );
        engine
            .apply(
                Command::CreateNoteWithId {
                    note_id: id.clone(),
                    title: "one".into(),
                    content: body,
                    folder_id: None,
                },
                11,
            )
            .unwrap();
        engine
            .apply(
                Command::CreateNote {
                    title: "two".into(),
                    content: format!("![shared](studyapp-media:{id}/shared.gif) ![relative](../images/{id}/relative.gif)"),
                },
                12,
            )
            .unwrap();
        let images = root.join("images").join(&id);
        let index = root.join("image-index").join(&id);
        fs::create_dir_all(&images).unwrap();
        fs::create_dir_all(&index).unwrap();
        for name in ["one.gif", "shared.gif", "relative.gif"] {
            let image = images.join(name);
            fs::write(&image, name).unwrap();
            fs::write(
                index.join(format!("{name}.json")),
                serde_json::to_vec(&image).unwrap(),
            )
            .unwrap();
        }
        let alias = images.join(format!(".studyapp-image-{}.pending", uuid::Uuid::new_v4()));
        fs::hard_link(images.join("one.gif"), &alias).unwrap();
        let backup = root.join(format!(
            "studyapp-before-restore-12-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        engine.apply(Command::Backup(backup.clone()), 13).unwrap();
        let manual = root.join("my-library.sqlite");
        fs::copy(&backup, &manual).unwrap();
        let p = prepare(&engine.snapshot(), &id).unwrap();
        assert_eq!(p.shared, 2);
        assert!(p.files.contains_key(&fs::canonicalize(&alias).unwrap()));
        assert_eq!(p.backup_count(), 1);
        fs::write(images.join("one.gif"), "changed").unwrap();
        assert!(execute(&mut engine, &p, 14).is_err());
        assert!(engine.snapshot().notes.iter().any(|n| n.id == id));
        let p = prepare(&engine.snapshot(), &id).unwrap();
        execute(&mut engine, &p, 14).unwrap();
        assert!(images.join("shared.gif").is_file() && index.join("shared.gif.json").is_file());
        assert!(images.join("relative.gif").is_file());
        assert!(
            studyapp_core::inspect_purge_backup(&manual)
                .unwrap()
                .unwrap()
                .notes
                .iter()
                .any(|n| n.id == id)
        );
        assert!(!alias.exists() && !images.join("one.gif").exists());
        assert!(
            studyapp_core::inspect_purge_backup(&backup)
                .unwrap()
                .unwrap()
                .notes
                .iter()
                .all(|n| n.id != id)
        );
    }
    #[test]
    fn purge_removes_retained_versions_and_keeps_another_note() {
        let root = tempfile::tempdir().unwrap().keep();
        let mut engine = Engine::open(&root.join("studyapp.sqlite3"), 100).unwrap();
        for (title, content) in [("first", "secret-one"), ("other", "keep-me")] {
            engine
                .apply(
                    Command::CreateNote {
                        title: title.into(),
                        content: content.into(),
                    },
                    101,
                )
                .unwrap();
        }
        let snap = engine.snapshot();
        let id = snap.notes[0].id.clone();
        engine
            .apply(
                Command::SaveNote {
                    note_id: id.clone(),
                    expected_hash: snap.versions[0].hash.clone(),
                    content: "secret-two".into(),
                },
                102,
            )
            .unwrap();
        let plan = prepare(&engine.snapshot(), &id).unwrap();
        assert!(plan.files.len() >= 4);
        execute(&mut engine, &plan, 103).unwrap();
        assert!(plan.files.keys().all(|p| !p.exists()));
        let snap = engine.snapshot();
        assert_eq!(snap.notes.len(), 1);
        assert_eq!(snap.versions[0].content, "keep-me");
        assert!(suppressed(&snap).unwrap().contains(&id));
    }

    #[test]
    fn interrupted_backup_scrub_is_retried_and_unproven_files_are_preserved() {
        let root = tempfile::tempdir().unwrap().keep();
        let mut engine = Engine::open(&root.join("studyapp.sqlite3"), 1).unwrap();
        engine
            .apply(
                Command::CreateNote {
                    title: "target".into(),
                    content: "identical".into(),
                },
                2,
            )
            .unwrap();
        let snapshot = engine.snapshot();
        let id = &snapshot.notes[0].id;
        let backup = root.join(format!(
            "studyapp-before-restore-2-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        engine.apply(Command::Backup(backup.clone()), 3).unwrap();
        let pending = root
            .join("notes")
            .join(format!(".pending-{}.md", uuid::Uuid::new_v4()));
        fs::write(&pending, "identical").unwrap();
        let draft = DraftRecord::new(
            NoteDraft {
                note_id: None,
                expected_hash: None,
                folder_id: None,
                asset_group: String::new(),
                title: "separate".into(),
                content: "identical".into(),
            },
            3,
        );
        let dp = DraftStore::record_path(&snapshot.data_path, &draft.id).unwrap();
        fs::create_dir_all(dp.parent().unwrap()).unwrap();
        fs::write(&dp, serde_json::to_vec(&draft).unwrap()).unwrap();
        let images = root.join("images").join(id);
        fs::create_dir_all(&images).unwrap();
        let orphan = images.join(format!(".studyapp-image-{}.pending", uuid::Uuid::new_v4()));
        fs::write(&orphan, "uncertain image").unwrap();
        let plan = prepare(&snapshot, id).unwrap();
        assert!(
            plan.unresolved
                .iter()
                .any(|p| p.file_name() == pending.file_name())
        );
        assert!(
            plan.unresolved
                .iter()
                .any(|p| p.file_name() == orphan.file_name())
        );

        // Simulate a crash after the logical backup deletion and before the
        // controller can record a successful checkpoint/VACUUM.
        let journals = root.join(".purged-notes");
        fs::create_dir_all(&journals).unwrap();
        let outsider = root.join("unrelated.txt");
        fs::write(&outsider, "unrelated").unwrap();
        let inventory = Inventory {
            files: BTreeMap::from([(outsider.clone(), stamp(&outsider).unwrap())]),
            backups: plan.backups,
        };
        fs::write(
            journals.join(format!("scope-{id}-{}.json", uuid::Uuid::new_v4())),
            serde_json::to_vec(&inventory).unwrap(),
        )
        .unwrap();
        studyapp_core::purge_managed_note_from_backup(&backup, id).unwrap();
        let retry = prepare(&engine.snapshot(), id).unwrap();
        assert_eq!(retry.backup_count(), 1);
        execute(&mut engine, &retry, 4).unwrap();
        for keep in [outsider, pending, orphan, dp] {
            assert!(keep.is_file());
        }
    }
}
