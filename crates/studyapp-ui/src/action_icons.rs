//! Explicit operation vocabulary. Values, navigation names, menu entries and
//! destructive confirmation sentences are deliberately not inferred as actions.
use gpui_kit::assets::IconName;

pub fn action_icon(label: &str) -> Option<IconName> {
    Some(match label {
        "暂停" | "暂停计时" => IconName::Pause,
        "开始计时" | "继续计时" | "继续" => IconName::Play,
        "结束" | "结束计时" => IconName::Square,
        "重新开始"
        | "重置"
        | "恢复本页默认"
        | "恢复本页默认草稿"
        | "恢复 Markdown 默认样式"
        | "使用默认位置" => IconName::RotateCcw,
        "记录这段" | "记录学习" | "补记一次学习" | "填写学习内容" => {
            IconName::NotebookPen
        }
        "设置计时" | "外观" | "设置" | "查看设置" | "查看日志设置" => {
            IconName::Settings
        }
        "在桌面显示" | "显示桌面小组件" | "隐藏桌面小组件" => {
            IconName::PanelsTopLeft
        }
        "主界面" => IconName::House,
        "置顶" => IconName::Pin,
        "取消置顶" => IconName::PinOff,
        "关闭" | "关闭弹窗" | "关闭通知" | "清除" => IconName::Close,
        "退出应用" => IconName::LogOut,
        "编辑" | "继续编辑" => IconName::Pencil,
        "新建笔记" | "+ 新建" => IconName::FilePlus,
        "导入文件" | "选择 Markdown 笔记" => IconName::FileInput,
        "阅读" | "预览" | "显示预览" => IconName::Eye,
        "隐藏预览" => IconName::EyeOff,
        "历史" | "历史与恢复" | "查看更新记录" | "查看维护记录" => {
            IconName::ClockArrowLeft
        }
        "更多操作" | "正文操作" | "笔记操作" => IconName::Ellipsis,
        "保存" | "保存修改" | "保存设置" | "保存学习记录" | "保存全部设置" => {
            IconName::Save
        }
        "收起" => IconName::ChevronUp,
        "展开" | "再显示 50 条" => IconName::ChevronDown,
        "上一页" => IconName::ChevronLeft,
        "下一页" => IconName::ChevronRight,
        "撤销最近一次复习" => IconName::Undo2,
        "检查更新"
        | "测试连接"
        | "测试图片云端连接"
        | "刷新统计"
        | "立即同步"
        | "重试同步"
        | "重试" => IconName::RefreshCw,
        "下载更新" | "查看发行文件" | "导出备份" => IconName::Download,
        "重启并安装" => IconName::PackageCheck,
        "查看更新" | "重试更新" | "查看更新内容" => IconName::ListChecks,
        "项目主页" => IconName::Github,
        "使用指南" => IconName::BookOpen,
        "MIT 许可" => IconName::FileText,
        "发布页面" | "打开发布页面" => IconName::ExternalLink,
        "定位文件"
        | "定位旧版本备份"
        | "选择文件夹"
        | "选择目录"
        | "选择资料库文件夹"
        | "更改位置"
        | "更改应用位置" => IconName::FolderOpen,
        "创建托管副本" | "复制版本" => IconName::Copy,
        "从备份恢复" => IconName::ArchiveRestore,
        "查看同步配置" | "图片存储设置" => IconName::Settings,
        "重试草稿读取" | "重试图片上传" | "重试同步并恢复输入" => {
            IconName::RefreshCw
        }
        "查看计时" => IconName::Timer,
        "查看复习" => IconName::BookOpen,
        "查看笔记库" => IconName::FolderOpen,
        "查看未保存内容" => IconName::Save,
        "查看同步" => IconName::RefreshCw,
        "查看存储" => IconName::HardDrive,
        "查看详情" | "详情" => IconName::Info,
        "编辑路径" => IconName::Pencil,
        "显示路径" => IconName::Eye,
        "隐藏路径" => IconName::EyeOff,
        "检查文件完整性" | "查看检查结果" => IconName::ShieldCheck,
        "应用到界面" | "应用" => IconName::Check,
        "+" | "＋" => IconName::Plus,
        "−" => IconName::Minus,
        "标题" => IconName::Heading,
        "粗体" => IconName::Bold,
        "斜体" => IconName::Italic,
        "列表" => IconName::List,
        "引用" => IconName::Quote,
        "代码" => IconName::Code,
        "表格" => IconName::Table,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn operation_vocabulary_never_guesses_values_or_confirmation_sentences() {
        for label in [
            "暂停",
            "继续计时",
            "置顶",
            "取消置顶",
            "保存",
            "标题",
            "查看更新内容",
        ] {
            assert!(action_icon(label).is_some(), "{label}");
        }
        for label in [
            "确认重新开始",
            "彻底删除",
            "确认删除所列旧文件",
            "取消",
            "25 分钟",
            "紧凑",
            "倒计时",
            "test",
            "学习设置",
        ] {
            assert!(action_icon(label).is_none(), "{label}");
        }
        assert_ne!(action_icon("置顶"), action_icon("取消置顶"));
        assert_ne!(action_icon("暂停"), action_icon("继续计时"));
    }
}
