use super::{Plan, keys_in};
use crate::sync_runtime::SyncRuntime;
use anyhow::{Context, Result, ensure};
use studyapp_core::{Engine, Snapshot};
use studyapp_sync::RemoteRemoval;

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Scope {
    pub sync: bool,
    pub cloud: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preview {
    pub local: Plan,
    pub scope: Scope,
    pub remote_note: Option<RemoteRemoval>,
    pub remote_images: Vec<RemoteRemoval>,
    pub remote_shared: usize,
    sync_target: crate::sync_runtime::SyncTarget,
}
impl Preview {
    pub fn objects(&self) -> impl Iterator<Item = &RemoteRemoval> {
        self.remote_note.iter().chain(self.remote_images.iter())
    }
}

pub fn preview(s: &Snapshot, runtime: &SyncRuntime, id: &str, scope: Scope) -> Result<Preview> {
    let local = super::prepare(s, id)?;
    let mut remote_note = None;
    let mut remote_images = vec![];
    let mut remote_shared = 0;
    if scope.sync || scope.cloud {
        let provider = runtime.media_provider_factory()()?;
        let mut protected = std::collections::BTreeSet::new();
        if scope.sync {
            let p = provider.as_ref().context("请先配置笔记同步目标")?;
            if p.read(id)?.is_some() {
                remote_note = Some(p.inspect_note(id)?);
            }
        }
        if scope.cloud {
            let assets = runtime.image_provider_factory(&s.settings.images)()?
                .context("请先配置图片云存储")?;
            if let Some(p) = provider {
                for other in p.list()? {
                    if (other != id || !scope.sync)
                        && let Some(note) = p.read(&other)?
                    {
                        protected.extend(keys_in(&note.content));
                    }
                }
            }
            for key in &local.keys {
                if protected.contains(key) {
                    remote_shared += 1;
                    continue;
                }
                if assets.get_asset(key)?.is_some() {
                    remote_images.push(assets.inspect_asset(key)?);
                }
            }
        }
    }
    Ok(Preview {
        local,
        scope,
        remote_note,
        remote_images,
        remote_shared,
        sync_target: runtime.status.settings.target.clone(),
    })
}

pub fn execute(
    engine: &mut Engine,
    runtime: &SyncRuntime,
    expected: &Preview,
    now: i64,
) -> Result<String> {
    let snapshot = engine.snapshot();
    let fresh = preview(&snapshot, runtime, &expected.local.note_id, expected.scope)?;
    ensure!(&fresh == expected, "删除范围或内容已变化，请重新预览");
    engine.verify_purge_ready()?;
    let mut results = Vec::new();
    if let Some(note) = &expected.remote_note {
        let p = runtime.media_provider_factory()()?.context("同步目标已改变")?;
        let result = verified(p.delete_note(note)?, &note.target)?;
        results.push(result.residual);
    }
    if !expected.remote_images.is_empty() {
        let p = runtime.image_provider_factory(&snapshot.settings.images)()?
            .context("云端图片目标已改变")?;
        for image in &expected.remote_images {
            let result = p.delete_asset(image).with_context(|| {
                format!("已有 {} 个远端项目完成删除，本机尚未清理", results.len())
            })?;
            results.push(verified(result, &image.target)?.residual);
        }
    }
    super::execute(engine, &expected.local, now)?;
    let mut msg = format!(
        "已删除本机托管笔记及 {} 个已确认文件，保留 {} 张共享图片。",
        expected.local.files.len(),
        expected.local.shared
    );
    if !expected.local.unresolved.is_empty() {
        msg.push_str(&format!(
            "有 {} 个文件无法确认归属，已保留；清理范围不完整。",
            expected.local.unresolved.len()
        ));
    }
    if !results.is_empty() {
        msg.push_str(&format!("已移除 {} 个远端当前对象。", results.len()));
        if results
            .iter()
            .any(|r| *r != studyapp_sync::RemovalResidual::NoneKnown)
        {
            msg.push_str("远端可能保留历史副本。");
        }
    }
    Ok(msg)
}
fn verified(
    outcome: studyapp_sync::RemovalOutcome,
    target: &str,
) -> Result<studyapp_sync::RemovalOutcome> {
    use studyapp_sync::RemovalState;
    match outcome.state {
        RemovalState::VerifiedAbsent => Ok(outcome),
        RemovalState::AcknowledgedUnverified => {
            anyhow::bail!("远端已接受删除 {target}，但无法验证是否删除完成。请检查远端后重新预览。")
        }
        RemovalState::Partial => {
            let mut message = format!(
                "远端仅完成部分删除 {target}，仍有 {} 个已确认文件待处理。请重新预览。",
                outcome.remaining_confirmed.len()
            );
            if !outcome.recovery_paths.is_empty() {
                message.push_str(&format!(
                    "已保全的文件位置：{}",
                    outcome.recovery_paths.join("、")
                ));
            }
            anyhow::bail!(message)
        }
    }
}
