use anyhow::{Result, ensure};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntryMode {
    Normal,
    UninstallInteractive,
    MaintenanceApply(String),
}

/// Arguments exclude argv[0]. A data directory selects read-only discovery;
/// it never supplies deletion authority.
pub fn parse_entry_mode(executable_basename: &str, args: &[String]) -> Result<EntryMode> {
    let alias = executable_basename.eq_ignore_ascii_case("Uninstall.exe");
    let uninstall = args.iter().any(|arg| arg == "--uninstall");
    let apply = args.iter().position(|arg| arg == "--maintenance-apply");
    ensure!(!(uninstall && apply.is_some()), "维护入口参数冲突");
    if let Some(index) = apply {
        ensure!(!alias && args.len() == 2 && index == 0, "内部维护参数无效");
        let reference = &args[1];
        ensure!(
            reference.len() == 32
                && reference
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "内部事务引用无效"
        );
        return Ok(EntryMode::MaintenanceApply(reference.clone()));
    }
    if alias || uninstall {
        maintenance_data_directory(args)?;
        return Ok(EntryMode::UninstallInteractive);
    }
    ensure!(
        !args.iter().any(|a| a.starts_with("--maintenance")
            || a.starts_with("--uninstall")
            || a.starts_with("--delete")
            || a.starts_with("--database")),
        "未知维护参数"
    );
    Ok(EntryMode::Normal)
}

pub fn maintenance_data_directory(args: &[String]) -> Result<Option<PathBuf>> {
    let mut explicit = None;
    let mut uninstall = false;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--uninstall" if !uninstall => uninstall = true,
            "--data-dir" if explicit.is_none() => {
                let path =
                    PathBuf::from(args.next().ok_or_else(|| anyhow::anyhow!("缺少隔离目录"))?);
                super::absolute_components(&path)?;
                explicit = Some(path);
            }
            _ => anyhow::bail!("卸载入口参数无效"),
        }
    }
    Ok(explicit)
}
