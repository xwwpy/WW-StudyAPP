use gpui_kit::{AssetSource, SharedString};
use std::borrow::Cow;

gpui_kit::assets::icon_assets!(
    ExtraIcons,
    [
        ArchiveRestore,
        ArchiveX,
        ArrowUp,
        ArrowUpRight,
        Bold,
        BookOpen,
        ChartBar,
        Check,
        ChevronDown,
        ChevronLeft,
        ChevronRight,
        ChevronUp,
        ChevronsUpDown,
        CircleAlert,
        CircleCheck,
        ClockArrowLeft,
        Close,
        Code,
        Copy,
        Download,
        Ellipsis,
        ExternalLink,
        Eye,
        EyeOff,
        FileInput,
        FilePlus,
        FileText,
        Files,
        Folder,
        FolderClosed,
        FolderOpen,
        Github,
        GripVertical,
        HardDrive,
        Heading,
        House,
        Inbox,
        Info,
        Italic,
        KeyRound,
        List,
        ListChecks,
        LogOut,
        Minus,
        NotebookPen,
        PackageCheck,
        Palette,
        PanelLeft,
        PanelsTopLeft,
        Pause,
        Pencil,
        Pin,
        PinOff,
        Play,
        Plus,
        Quote,
        RefreshCw,
        RotateCcw,
        Save,
        Settings,
        ShieldCheck,
        Square,
        Table,
        Timer,
        Trash,
        Undo2,
        WindowClose,
        WindowMaximize,
        WindowMinimize,
        WindowRestore
    ]
);

pub struct StudyAssets;
impl AssetSource for StudyAssets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        if path == "images/app-icon.png" {
            return Ok(Some(Cow::Borrowed(include_bytes!(
                "../../../assets/icon.png"
            ))));
        }
        match ExtraIcons.load(path)? {
            Some(icon) => Ok(Some(icon)),
            None => gpui_kit::assets::Assets.load(path),
        }
    }
    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::StudyAssets;
    use gpui_kit::{AssetSource, assets::IconName};
    #[test]
    fn new_operation_icons_are_embedded_for_offline_native_windows() {
        for icon in [
            IconName::Pin,
            IconName::PinOff,
            IconName::GripVertical,
            IconName::NotebookPen,
            IconName::ArchiveX,
            IconName::ArrowUpRight,
            IconName::Trash,
            IconName::KeyRound,
            IconName::Save,
            IconName::Files,
        ] {
            let path = icon.path();
            let bytes = StudyAssets.load(&path).unwrap().unwrap();
            assert!(
                std::str::from_utf8(&bytes).unwrap().contains("<svg"),
                "{path}"
            );
        }
    }
}
