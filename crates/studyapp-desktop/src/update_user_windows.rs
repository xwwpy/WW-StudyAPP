//! Same-user, same-session ordinary launch from an accidentally elevated app.
//! No shell identity substitution, credentials, or desktop ACL changes.
use super::{Handle, NO_WINDOW, Result, io};
use std::{
    mem::{size_of, size_of_val},
    os::windows::{ffi::OsStrExt, process::CommandExt},
    path::Path,
    process::{Child, Command, Stdio},
};
use windows_sys::Win32::{
    Foundation::{HANDLE, STILL_ACTIVE},
    Security::{
        CreateRestrictedToken, CreateWellKnownSid, DISABLE_MAX_PRIVILEGE, GetLengthSid,
        GetTokenInformation, SID_AND_ATTRIBUTES, SetTokenInformation, TOKEN_ADJUST_DEFAULT,
        TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_ELEVATION, TOKEN_INFORMATION_CLASS,
        TOKEN_LINKED_TOKEN, TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TokenElevation,
        TokenIntegrityLevel, TokenLinkedToken, TokenSessionId, TokenUser,
        WinBuiltinAdministratorsSid, WinMediumLabelSid,
    },
    System::Threading::{
        CreateProcessAsUserW, CreateProcessWithTokenW, GetCurrentProcess, GetExitCodeProcess,
        OpenProcessToken, PROCESS_INFORMATION, STARTUPINFOW, TerminateProcess,
    },
};

fn os_error() -> String {
    std::io::Error::last_os_error().to_string()
}
fn information<T: Default>(token: HANDLE, class: TOKEN_INFORMATION_CLASS) -> Result<T> {
    let mut value = T::default();
    let mut length = 0;
    if unsafe {
        GetTokenInformation(
            token,
            class,
            &mut value as *mut _ as *mut _,
            size_of::<T>() as u32,
            &mut length,
        )
    } == 0
    {
        return Err(os_error());
    }
    Ok(value)
}
fn sid(token: HANDLE, class: TOKEN_INFORMATION_CLASS) -> Result<Vec<u8>> {
    let mut buffer = [0usize; 128];
    let mut length = 0;
    if unsafe {
        GetTokenInformation(
            token,
            class,
            buffer.as_mut_ptr().cast(),
            size_of_val(&buffer) as u32,
            &mut length,
        )
    } == 0
    {
        return Err(os_error());
    }
    let value = unsafe { &*buffer.as_ptr().cast::<SID_AND_ATTRIBUTES>() };
    let len = unsafe { GetLengthSid(value.Sid) } as usize;
    if !(12..=68).contains(&len) {
        return Err("Invalid update token SID".into());
    }
    Ok(unsafe { std::slice::from_raw_parts(value.Sid.cast::<u8>(), len) }.to_vec())
}
fn validate(original: HANDLE, selected: HANDLE) -> Result<()> {
    let elevation: TOKEN_ELEVATION = information(selected, TokenElevation)?;
    let integrity = sid(selected, TokenIntegrityLevel)?;
    let level = u32::from_le_bytes(integrity[integrity.len() - 4..].try_into().unwrap());
    if elevation.TokenIsElevated != 0
        || level > 8192
        || sid(original, TokenUser)? != sid(selected, TokenUser)?
        || information::<u32>(original, TokenSessionId)?
            != information::<u32>(selected, TokenSessionId)?
    {
        return Err("无法建立同一用户的普通权限更新进程；尚未开始安装。".into());
    }
    Ok(())
}
struct Token {
    current: Handle,
    selected: Option<Handle>,
    restricted: bool,
}
impl Token {
    fn acquire() -> Result<Self> {
        let mut current = std::ptr::null_mut();
        if unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_DEFAULT,
                &mut current,
            )
        } == 0
        {
            return Err(os_error());
        }
        let current = Handle(current);
        let elevation: TOKEN_ELEVATION = information(current.0, TokenElevation)?;
        if elevation.TokenIsElevated == 0 {
            return Ok(Self {
                current,
                selected: None,
                restricted: false,
            });
        }
        let linked: std::result::Result<TOKEN_LINKED_TOKEN, _> =
            information(current.0, TokenLinkedToken);
        let (selected, restricted) = if let Ok(linked) = linked {
            (Handle(linked.LinkedToken), false)
        } else {
            // UAC-disabled sessions have no linked token. Remove admin authority
            // and lower only a newly restricted token, never the caller token.
            let mut admin = [0u32; 17];
            let mut length = size_of_val(&admin) as u32;
            if unsafe {
                CreateWellKnownSid(
                    WinBuiltinAdministratorsSid,
                    std::ptr::null_mut(),
                    admin.as_mut_ptr().cast(),
                    &mut length,
                )
            } == 0
            {
                return Err(os_error());
            }
            let disabled = SID_AND_ATTRIBUTES {
                Sid: admin.as_mut_ptr().cast(),
                Attributes: 0,
            };
            let mut restricted = std::ptr::null_mut();
            if unsafe {
                CreateRestrictedToken(
                    current.0,
                    DISABLE_MAX_PRIVILEGE,
                    1,
                    &disabled,
                    0,
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    &mut restricted,
                )
            } == 0
            {
                return Err(os_error());
            }
            let restricted = Handle(restricted);
            let mut medium = [0u32; 17];
            let mut length = size_of_val(&medium) as u32;
            if unsafe {
                CreateWellKnownSid(
                    WinMediumLabelSid,
                    std::ptr::null_mut(),
                    medium.as_mut_ptr().cast(),
                    &mut length,
                )
            } == 0
            {
                return Err(os_error());
            }
            let label = TOKEN_MANDATORY_LABEL {
                Label: SID_AND_ATTRIBUTES {
                    Sid: medium.as_mut_ptr().cast(),
                    Attributes: 0x20, // SE_GROUP_INTEGRITY
                },
            };
            if unsafe {
                SetTokenInformation(
                    restricted.0,
                    TokenIntegrityLevel,
                    &label as *const _ as *const _,
                    size_of::<TOKEN_MANDATORY_LABEL>() as u32 + length,
                )
            } == 0
            {
                return Err(os_error());
            }
            (restricted, true)
        };
        validate(current.0, selected.0)?;
        Ok(Self {
            current,
            selected: Some(selected),
            restricted,
        })
    }
}
pub fn ensure_available() -> Result<()> {
    Token::acquire().map(|_| ())
}

pub struct Supervisor {
    normal: Option<Child>,
    reduced: Option<Handle>,
}
impl Supervisor {
    pub fn exited(&mut self) -> Result<bool> {
        if let Some(child) = &mut self.normal {
            return io(child.try_wait()).map(|value| value.is_some());
        }
        let handle = self.reduced.as_ref().ok_or("Missing supervisor process")?;
        let mut code = 0;
        if unsafe { GetExitCodeProcess(handle.0, &mut code) } == 0 {
            return Err(os_error());
        }
        Ok(code != STILL_ACTIVE as u32)
    }
    pub fn kill(&mut self) {
        if let Some(child) = &mut self.normal {
            let _ = child.kill();
        }
        if let Some(handle) = &self.reduced {
            unsafe {
                TerminateProcess(handle.0, 1);
            }
        }
    }
}
fn wide(value: &std::ffi::OsStr) -> Result<Vec<u16>> {
    let mut result: Vec<u16> = value.encode_wide().collect();
    if result.contains(&0) {
        return Err("Invalid NUL in update launch path".into());
    }
    result.push(0);
    Ok(result)
}
// Both arguments are canonical paths: no embedded quotes or trailing slash.
// No shell interprets spaces, Unicode, ampersands or command metacharacters.
pub fn spawn(dir: &Path) -> Result<Supervisor> {
    let token = Token::acquire()?;
    let executable = dir.join("supervisor.exe");
    let Some(selected) = &token.selected else {
        return io(Command::new(executable)
            .arg("--update-supervisor")
            .arg(dir)
            .current_dir(dir)
            .creation_flags(NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn())
        .map(|child| Supervisor {
            normal: Some(child),
            reduced: None,
        });
    };
    validate(token.current.0, selected.0)?;
    let path = executable
        .to_str()
        .ok_or("Invalid supervisor Unicode path")?;
    let directory = dir.to_str().ok_or("Invalid update Unicode directory")?;
    if path.contains('"') || directory.contains('"') || directory.ends_with('\\') {
        return Err("Invalid update launch arguments".into());
    }
    let application = wide(executable.as_os_str())?;
    let working = wide(dir.as_os_str())?;
    let mut command = wide(std::ffi::OsStr::new(&format!(
        "\"{path}\" --update-supervisor \"{directory}\""
    )))?;
    let startup = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut process = PROCESS_INFORMATION::default();
    let launched = unsafe {
        if token.restricted {
            CreateProcessAsUserW(
                selected.0,
                application.as_ptr(),
                command.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                NO_WINDOW,
                std::ptr::null(),
                working.as_ptr(),
                &startup,
                &mut process,
            )
        } else {
            CreateProcessWithTokenW(
                selected.0,
                0,
                application.as_ptr(),
                command.as_mut_ptr(),
                NO_WINDOW,
                std::ptr::null(),
                working.as_ptr(),
                &startup,
                &mut process,
            )
        }
    };
    if launched == 0 {
        return Err(format!("无法启动普通权限更新进程：{}", os_error()));
    }
    let _thread = Handle(process.hThread);
    Ok(Supervisor {
        normal: None,
        reduced: Some(Handle(process.hProcess)),
    })
}

#[cfg(feature = "update-acceptance")]
pub fn record_process(dir: &Path, role: &str, pid: u32) -> Result<()> {
    if !crate::update_acceptance::active() {
        return Ok(());
    }
    let raw = unsafe { windows_sys::Win32::System::Threading::OpenProcess(0x1000, 0, pid) };
    if raw.is_null() {
        return Err(os_error());
    }
    let process = Handle(raw);
    let mut raw = std::ptr::null_mut();
    if unsafe { OpenProcessToken(process.0, TOKEN_QUERY, &mut raw) } == 0 {
        return Err(os_error());
    }
    let token = Handle(raw);
    let original = Token::acquire()?;
    validate(original.current.0, token.0)?;
    let user = sid(token.0, TokenUser)?;
    let authority = user[2..8]
        .iter()
        .fold(0u64, |value, byte| (value << 8) | u64::from(*byte));
    let mut user_sid = format!("S-{}-{authority}", user[0]);
    for sub in user[8..].as_chunks::<4>().0 {
        user_sid.push_str(&format!("-{}", u32::from_le_bytes(*sub)));
    }
    let integrity = sid(token.0, TokenIntegrityLevel)?;
    let level = u32::from_le_bytes(integrity[integrity.len() - 4..].try_into().unwrap());
    let value = serde_json::json!({"pid":pid,"role":role,"user_sid":user_sid,
        "elevated":false,"integrity_level":level,"session_id":information::<u32>(token.0, TokenSessionId)?});
    super::write_new(
        &dir.join(format!("{role}-token.json")),
        &serde_json::to_vec(&value).map_err(|error| error.to_string())?,
    )
}
