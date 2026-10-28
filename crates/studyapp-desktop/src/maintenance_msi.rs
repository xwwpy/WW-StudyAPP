//! Windows Installer owns every installed-resource transaction.
use super::*;

pub(super) fn verify_registration(receipt: &InstallReceipt) -> Result<()> {
    let _guards = checked_root(receipt)?;
    let current = runtime::registered_current_user_msi()?;
    ensure!(
        runtime::matching_msi_owner(&receipt.install_root.join("WW-StudyApp.exe"), &current)?
            .as_ref()
            == Some(&receipt.owner),
        "Windows Installer 所有者已改变"
    );
    ensure!(
        current.iter().any(
            |r| same_absolute_path(&r.install_root, &receipt.install_root).unwrap_or(false)
                && r.version == receipt.version
        ),
        "Windows Installer 版本已改变"
    );
    Ok(())
}
#[cfg(windows)]
pub(super) fn package_property(path: &Path, name: &str) -> Result<String> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "msi")]
    unsafe extern "system" {
        fn MsiOpenDatabaseW(path: *const u16, persist: *const u16, database: *mut u32) -> u32;
        fn MsiDatabaseOpenViewW(database: u32, query: *const u16, view: *mut u32) -> u32;
        fn MsiViewExecute(view: u32, record: u32) -> u32;
        fn MsiViewFetch(view: u32, record: *mut u32) -> u32;
        fn MsiRecordGetStringW(record: u32, field: u32, value: *mut u16, length: *mut u32) -> u32;
        fn MsiCloseHandle(handle: u32) -> u32;
    }
    struct Handle(u32);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                MsiCloseHandle(self.0);
            }
        }
    }
    ensure!(
        ["ProductVersion", "ProductCode", "UpgradeCode", "ALLUSERS"].contains(&name),
        "不支持的 MSI 属性"
    );
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut database = 0;
    // SAFETY: validated NUL-terminated path and writable output; null persistence is read-only.
    ensure!(
        unsafe { MsiOpenDatabaseW(path.as_ptr(), std::ptr::null(), &mut database) } == 0,
        "无法检查 MSI 数据库"
    );
    let database = Handle(database);
    let query: Vec<u16> = format!("SELECT `Value` FROM `Property` WHERE `Property`='{name}'")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut view = 0;
    ensure!(
        unsafe { MsiDatabaseOpenViewW(database.0, query.as_ptr(), &mut view) } == 0,
        "无法检查 MSI 属性"
    );
    let view = Handle(view);
    ensure!(
        unsafe { MsiViewExecute(view.0, 0) } == 0,
        "无法读取 MSI 属性"
    );
    let mut record = 0;
    let status = unsafe { MsiViewFetch(view.0, &mut record) };
    if status == 259 {
        return Ok(String::new());
    }
    ensure!(status == 0, "无法读取 MSI 属性");
    let record = Handle(record);
    let mut value = [0u16; 1024];
    let mut length = value.len() as u32;
    ensure!(
        unsafe { MsiRecordGetStringW(record.0, 1, value.as_mut_ptr(), &mut length) } == 0
            && length < value.len() as u32,
        "MSI 属性过长"
    );
    Ok(String::from_utf16(&value[..length as usize])?)
}
#[cfg(not(windows))]
pub(super) fn package_property(_path: &Path, _name: &str) -> Result<String> {
    bail!("Windows Installer 仅支持 Windows")
}

pub(super) fn verify_package(path: &Path, version: &str) -> Result<String> {
    ensure!(
        package_property(path, "ProductVersion")? == version,
        "MSI 版本不符"
    );
    ensure!(
        package_property(path, "UpgradeCode")?.eq_ignore_ascii_case(MSI_UPGRADE_CODE),
        "MSI 产品不符"
    );
    let product_code = package_property(path, "ProductCode")?;
    ensure!(valid_product_code(&product_code), "MSI 产品标识无效");
    ensure!(
        package_property(path, "ALLUSERS")?.is_empty(),
        "MSI 安装范围无效"
    );
    Ok(product_code)
}
pub(super) fn planned_receipt(
    receipt: &InstallReceipt,
    product_code: &str,
    version: &str,
    hash: &str,
) -> Result<InstallReceipt> {
    let mut next = receipt.clone();
    let InstallOwner::Msi {
        product_code: target,
        ..
    } = &mut next.owner
    else {
        bail!("MSI 更新所有者无效");
    };
    *target = product_code.into();
    next.install_id = product_code.trim_matches(['{', '}']).into();
    next.version = version.into();
    next.source_package_sha256 = hash.into();
    verify_transition(receipt, &next)?;
    Ok(next)
}
pub(super) fn verify_transition(old: &InstallReceipt, next: &InstallReceipt) -> Result<()> {
    validate_receipt(old)?;
    validate_receipt(next)?;
    let (
        InstallOwner::Msi {
            product_code: old_product,
            upgrade_code: old_family,
            user_sid: old_sid,
        },
        InstallOwner::Msi {
            product_code: next_product,
            upgrade_code: next_family,
            user_sid: next_sid,
        },
    ) = (&old.owner, &next.owner)
    else {
        bail!("MSI 更新所有者无效");
    };
    ensure!(
        old.install_id == old_product.trim_matches(['{', '}'])
            && next.install_id == next_product.trim_matches(['{', '}'])
            && !old_product.eq_ignore_ascii_case(next_product)
            && old_family == next_family
            && old_sid == next_sid
            && old.root_identity == next.root_identity
            && same_absolute_path(&old.install_root, &next.install_root)?
            && semver::Version::parse(&next.version)? > semver::Version::parse(&old.version)?
            && valid_sha256(&next.source_package_sha256),
        "MSI 更新目标身份未绑定"
    );
    Ok(())
}
pub(super) fn verify_package_target(
    old: &InstallReceipt,
    next: &InstallReceipt,
    product_code: &str,
    hash: &str,
) -> Result<()> {
    verify_transition(old, next)?;
    ensure!(
        matches!(&next.owner, InstallOwner::Msi { product_code: expected, .. } if expected == product_code)
            && next.source_package_sha256 == hash,
        "MSI 安装包与目标身份不符"
    );
    Ok(())
}
pub(super) fn verify_target_receipt(
    expected: &InstallReceipt,
    observed: &InstallReceipt,
) -> Result<()> {
    validate_receipt(expected)?;
    validate_receipt(observed)?;
    ensure!(
        matches!(expected.owner, InstallOwner::Msi { .. })
            && expected.owner == observed.owner
            && expected.install_id == observed.install_id
            && expected.root_identity == observed.root_identity
            && same_absolute_path(&expected.install_root, &observed.install_root)?
            && expected.version == observed.version,
        "MSI 安装结果与目标身份不符"
    );
    Ok(())
}
#[cfg(windows)]
pub(super) fn invoke(argument: &str, target: &std::ffi::OsStr) -> Result<u32> {
    use std::os::windows::{ffi::OsStringExt, process::CommandExt};
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetSystemDirectoryW(buffer: *mut u16, size: u32) -> u32;
    }
    let mut directory = [0u16; 32768];
    let length = unsafe { GetSystemDirectoryW(directory.as_mut_ptr(), directory.len() as u32) };
    ensure!(
        length > 0 && length < directory.len() as u32,
        "系统目录无法验证"
    );
    let executable = PathBuf::from(std::ffi::OsString::from_wide(&directory[..length as usize]))
        .join("msiexec.exe");
    let status = std::process::Command::new(executable)
        .arg(argument)
        .arg(target)
        .args(["/passive", "/norestart"])
        .creation_flags(0x0800_0000)
        .stdin(std::process::Stdio::null())
        .status()?;
    status
        .code()
        .map(|n| n as u32)
        .context("Windows Installer 未返回状态")
}
#[cfg(not(windows))]
pub(super) fn invoke(_argument: &str, _target: &std::ffi::OsStr) -> Result<u32> {
    bail!("Windows Installer 仅支持 Windows")
}
