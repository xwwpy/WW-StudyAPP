//! Local named pipes with current-user ACLs and kernel-attested process peers.
use super::*;
use std::os::windows::{
    ffi::OsStrExt,
    io::{AsRawHandle, FromRawHandle, OwnedHandle},
};
use windows_sys::Win32::{
    Foundation::{INVALID_HANDLE_VALUE, LocalFree},
    Security::{
        Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW, SECURITY_ATTRIBUTES,
    },
    System::{
        Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId,
            GetNamedPipeServerProcessId, SetNamedPipeHandleState,
        },
        Threading::{
            GetCurrentProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
            QueryFullProcessImageNameW,
        },
    },
};
fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}
fn name(session: &str) -> Result<Vec<u16>> {
    ensure!(valid_id(session), "无效占用者会话");
    Ok(wide(std::ffi::OsStr::new(&format!(
        r"\\.\pipe\WWStudy-owner-{session}"
    ))))
}
fn os_check(value: i32) -> Result<()> {
    ensure!(value != 0, "{}", std::io::Error::last_os_error());
    Ok(())
}
pub(super) fn pending(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
        || matches!(error.raw_os_error(), Some(232 | 536))
}
struct Allocation(*mut std::ffi::c_void);
impl Drop for Allocation {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
pub(super) struct Listener {
    file: File,
}
impl Listener {
    pub fn bind(session: &str) -> Result<Self> {
        // Read-only user identity query; the pipe grants access only to this SID.
        let sid = crate::maintenance_handoff::process_user_sid(unsafe { GetCurrentProcess() })?;
        let sddl = wide(std::ffi::OsStr::new(&format!("D:P(A;;GA;;;{sid})")));
        let mut descriptor = std::ptr::null_mut();
        os_check(unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        })?;
        let _allocation = Allocation(descriptor);
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        // Duplex, first-instance only, byte stream, nonblocking, reject remote clients.
        let pipe = unsafe {
            CreateNamedPipeW(
                name(session)?.as_ptr(),
                3 | 0x80000,
                1 | 8,
                1,
                LIMIT as u32,
                LIMIT as u32,
                0,
                &attributes,
            )
        };
        ensure!(
            pipe != INVALID_HANDLE_VALUE && !pipe.is_null(),
            "无法创建本地占用者通道：{}",
            std::io::Error::last_os_error()
        );
        // CreateNamedPipe transfers the non-inheritable handle to this File.
        Ok(Self {
            file: unsafe { File::from_raw_handle(pipe) },
        })
    }
    pub fn accept(&mut self) -> Result<Option<Connection>> {
        let ok = unsafe { ConnectNamedPipe(self.file.as_raw_handle(), std::ptr::null_mut()) };
        if ok == 0 {
            let error = std::io::Error::last_os_error();
            match error.raw_os_error() {
                Some(536) => return Ok(None),
                Some(535) => {}
                Some(232) => {
                    unsafe {
                        DisconnectNamedPipe(self.file.as_raw_handle());
                    }
                    return Ok(None);
                }
                _ => return Err(error.into()),
            }
        }
        Ok(Some(Connection {
            file: self.file.try_clone()?,
            server: true,
        }))
    }
}
pub(super) struct Connection {
    file: File,
    server: bool,
}
impl Connection {
    pub fn connect(session: &str) -> Result<Self> {
        use std::os::windows::fs::OpenOptionsExt;
        let endpoint = name(session)?;
        let endpoint = String::from_utf16(&endpoint[..endpoint.len() - 1])?;
        let deadline = Instant::now() + DEADLINE;
        let file = loop {
            // Do not let even a local peer impersonate the requesting user.
            match std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .security_qos_flags(0x0010_0000)
                .open(&endpoint)
            {
                Ok(file) => break file,
                Err(error) if error.raw_os_error() == Some(231) && Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => return Err(error.into()),
            }
        };
        let mode = 1u32; // PIPE_NOWAIT; no unbounded GUI/background reads.
        os_check(unsafe {
            SetNamedPipeHandleState(
                file.as_raw_handle(),
                &mode,
                std::ptr::null(),
                std::ptr::null(),
            )
        })?;
        Ok(Self {
            file,
            server: false,
        })
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        if self.server {
            unsafe {
                DisconnectNamedPipe(self.file.as_raw_handle());
            }
        }
    }
}
impl Read for Connection {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self.file.read(buf) {
            // PIPE_NOWAIT byte streams can succeed with zero bytes when empty;
            // unlike a Unix socket this is not evidence that the peer exited.
            Ok(0) if !buf.is_empty() => Err(std::io::ErrorKind::WouldBlock.into()),
            result => result,
        }
    }
}
impl Write for Connection {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.file.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
pub(super) fn verify_peer(connection: &Connection) -> Result<u32> {
    let mut pid = 0;
    let query = if connection.server {
        GetNamedPipeClientProcessId
    } else {
        GetNamedPipeServerProcessId
    };
    os_check(unsafe { query(connection.file.as_raw_handle(), &mut pid) })?;
    verify_process(pid)
}
fn verify_process(pid: u32) -> Result<u32> {
    ensure!(pid > 0, "占用者进程无效");
    let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    ensure!(!raw.is_null(), "无法核实占用者进程");
    let process = unsafe { OwnedHandle::from_raw_handle(raw) };
    ensure!(
        crate::maintenance_handoff::process_user_sid(process.as_raw_handle())?
            == crate::maintenance_handoff::process_user_sid(unsafe { GetCurrentProcess() })?,
        "占用者属于其他账户"
    );
    let mut path = vec![0u16; 32768];
    let mut length = path.len() as u32;
    os_check(unsafe {
        QueryFullProcessImageNameW(process.as_raw_handle(), 0, path.as_mut_ptr(), &mut length)
    })?;
    ensure!(
        length > 0 && (length as usize) < path.len(),
        "无法核实占用者程序文件"
    );
    let actual = File::open(PathBuf::from(String::from_utf16(&path[..length as usize])?))?;
    let expected = File::open(std::env::current_exe()?)?;
    ensure!(
        identity(&actual)? == identity(&expected)?,
        "占用者并非同一安装的应用；请手动关闭"
    );
    Ok(pid)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            drop(self.0.stdin.take());
            let _ = self.0.wait();
        }
    }
    #[test]
    fn foreign_executable_is_rejected_even_under_the_same_user() {
        use std::os::windows::process::CommandExt;
        let child = std::process::Command::new("cmd.exe")
            .args(["/d", "/q", "/c", "set /p owner_probe="])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .creation_flags(0x0800_0000)
            .spawn()
            .unwrap();
        let mut child = Child(child);
        assert!(child.0.try_wait().unwrap().is_none());
        let error = verify_process(child.0.id()).unwrap_err().to_string();
        assert!(error.contains("并非同一安装"), "{error}");
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "verification must not close a foreign process"
        );
    }
}
