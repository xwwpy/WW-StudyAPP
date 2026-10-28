use super::{FileIdentity, UninstallPreview, same_absolute_path};
use anyhow::{Result, ensure};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmationStage {
    Selection,
    DatabaseReview,
    DatabaseFinal,
    Ready,
}
#[derive(Clone, Debug)]
pub struct UninstallConfirmation {
    preview: UninstallPreview,
    stage: ConfirmationStage,
}
/// Cannot be deserialized or constructed by a caller. Execution consumes it.
#[derive(Debug)]
pub struct ConfirmedUninstall {
    preview: UninstallPreview,
}
impl ConfirmedUninstall {
    pub fn preview(&self) -> &UninstallPreview {
        &self.preview
    }
    pub fn into_preview(self) -> UninstallPreview {
        self.preview
    }
    pub fn validate_database(&self, current: Option<(&Path, &FileIdentity)>) -> Result<()> {
        if !self.preview.selection.database {
            return Ok(());
        }
        let (path, identity) =
            current.ok_or_else(|| anyhow::anyhow!("数据库不可用，请重新确认"))?;
        ensure!(
            self.preview
                .database_path
                .as_deref()
                .map(|old| same_absolute_path(old, path))
                .transpose()?
                .unwrap_or(false)
                && self.preview.database_identity.as_ref() == Some(identity),
            "数据库身份已改变，请重新确认"
        );
        Ok(())
    }
}
impl UninstallConfirmation {
    pub fn new(preview: UninstallPreview) -> Self {
        Self {
            preview,
            stage: ConfirmationStage::Selection,
        }
    }
    pub fn stage(&self) -> ConfirmationStage {
        self.stage
    }
    pub fn preview(&self) -> &UninstallPreview {
        &self.preview
    }
    pub fn confirm_selection(&mut self) {
        if self.stage == ConfirmationStage::Selection {
            self.stage = if self.preview.selection.database {
                ConfirmationStage::DatabaseReview
            } else {
                ConfirmationStage::Ready
            };
        }
    }
    pub fn confirm_database_review(&mut self) -> Result<()> {
        ensure!(
            self.stage == ConfirmationStage::DatabaseReview,
            "数据库确认顺序无效"
        );
        self.stage = ConfirmationStage::DatabaseFinal;
        Ok(())
    }
    pub fn confirm_database_final(&mut self, phrase: &str) -> Result<()> {
        ensure!(
            self.stage == ConfirmationStage::DatabaseFinal,
            "数据库确认顺序无效"
        );
        ensure!(phrase == "删除数据库", "请输入完整确认文字");
        self.stage = ConfirmationStage::Ready;
        Ok(())
    }
    pub fn finish(self) -> Result<ConfirmedUninstall> {
        ensure!(self.stage == ConfirmationStage::Ready, "卸载尚未确认");
        Ok(ConfirmedUninstall {
            preview: self.preview,
        })
    }
}
