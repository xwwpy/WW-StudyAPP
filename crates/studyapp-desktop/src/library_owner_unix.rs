//! Private Unix sockets; peer credentials come from the kernel, not the hint.
use super::*;
use std::os::{
    fd::{AsRawFd, FromRawFd},
    unix::{
        fs::{FileTypeExt, MetadataExt},
        net::{UnixListener, UnixStream},
    },
};
fn directory() -> Result<PathBuf> {
    let path = std::env::temp_dir().join(format!("wwso-{}", unsafe { libc::geteuid() }));
    if !path.try_exists()? {
        match crate::maintenance_handoff::create_private_directory(&path) {
            Ok(()) => {}
            Err(e) if path.try_exists()? => {
                let _ = e;
            }
            Err(e) => return Err(e),
        }
    }
    let meta = std::fs::symlink_metadata(&path)?;
    ensure!(
        meta.is_dir()
            && !meta.file_type().is_symlink()
            && meta.uid() == unsafe { libc::geteuid() }
            && meta.mode() & 0o077 == 0,
        "本地占用者通道目录不安全"
    );
    Ok(path)
}
fn endpoint(session: &str) -> Result<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    ensure!(valid_id(session), "无效占用者会话");
    let path = directory()?.join(session);
    ensure!(
        path.as_os_str().as_bytes().len() < 104,
        "本地占用者通道路径过长"
    );
    Ok(path)
}
pub(super) fn pending(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
    )
}
pub(super) struct Listener {
    socket: UnixListener,
    path: PathBuf,
    id: (u64, u64),
}
impl Listener {
    pub fn bind(session: &str) -> Result<Self> {
        let path = endpoint(session)?;
        let socket = UnixListener::bind(&path)?;
        socket.set_nonblocking(true)?;
        let m = std::fs::symlink_metadata(&path)?;
        Ok(Self {
            socket,
            path,
            id: (m.dev(), m.ino()),
        })
    }
    pub fn accept(&mut self) -> Result<Option<Connection>> {
        match self.socket.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(true)?;
                Ok(Some(Connection(stream)))
            }
            Err(e) if pending(&e) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}
impl Drop for Listener {
    fn drop(&mut self) {
        if let Ok(m) = std::fs::symlink_metadata(&self.path)
            && m.file_type().is_socket()
            && (m.dev(), m.ino()) == self.id
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
pub(super) struct Connection(UnixStream);
impl Connection {
    pub fn connect(session: &str) -> Result<Self> {
        use std::os::unix::ffi::OsStrExt;
        let path = endpoint(session)?;
        let bytes = path.as_os_str().as_bytes();
        let raw = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
        ensure!(raw >= 0, "{}", std::io::Error::last_os_error());
        // Own immediately so all fcntl/connect/error exits close this descriptor.
        let stream = unsafe { UnixStream::from_raw_fd(raw) };
        stream.set_nonblocking(true)?;
        ensure!(
            unsafe { libc::fcntl(raw, libc::F_SETFD, libc::FD_CLOEXEC) } == 0,
            "{}",
            std::io::Error::last_os_error()
        );
        let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        address.sun_family = libc::AF_UNIX as libc::sa_family_t;
        ensure!(
            bytes.len() < address.sun_path.len(),
            "本地占用者通道路径过长"
        );
        for (to, from) in address.sun_path.iter_mut().zip(bytes) {
            *to = *from as libc::c_char;
        }
        let length = (std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1)
            as libc::socklen_t;
        #[cfg(target_os = "macos")]
        {
            address.sun_len = length as u8;
        }
        let result =
            unsafe { libc::connect(raw, (&address as *const libc::sockaddr_un).cast(), length) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EINPROGRESS) {
                return Err(error.into());
            }
            let deadline = Instant::now() + DEADLINE;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                ensure!(!remaining.is_zero(), "占用者通道连接超时");
                let mut poll = libc::pollfd {
                    fd: raw,
                    events: libc::POLLOUT,
                    revents: 0,
                };
                let result =
                    unsafe { libc::poll(&mut poll, 1, remaining.as_millis().max(1) as i32) };
                if result < 0 {
                    let error = std::io::Error::last_os_error();
                    if error.kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(error.into());
                }
                if result == 0 {
                    continue;
                }
                let mut error = 0i32;
                let mut size = size_of::<i32>() as libc::socklen_t;
                ensure!(
                    unsafe {
                        libc::getsockopt(
                            raw,
                            libc::SOL_SOCKET,
                            libc::SO_ERROR,
                            (&mut error as *mut i32).cast(),
                            &mut size,
                        )
                    } == 0,
                    "{}",
                    std::io::Error::last_os_error()
                );
                if error != 0 {
                    return Err(std::io::Error::from_raw_os_error(error).into());
                }
                break;
            }
        }
        Ok(Self(stream))
    }
}
impl Read for Connection {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(b)
    }
}
impl Write for Connection {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.write(b)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
#[cfg(target_os = "linux")]
pub(super) fn verify_peer(connection: &Connection) -> Result<u32> {
    let mut peer = std::mem::MaybeUninit::<libc::ucred>::zeroed();
    let mut size = size_of::<libc::ucred>() as libc::socklen_t;
    ensure!(
        unsafe {
            libc::getsockopt(
                connection.0.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                peer.as_mut_ptr().cast(),
                &mut size,
            )
        } == 0
            && size as usize == size_of::<libc::ucred>(),
        "无法核实占用者通道身份"
    );
    let peer = unsafe { peer.assume_init() };
    ensure!(
        peer.pid > 0 && peer.uid == unsafe { libc::geteuid() },
        "占用者属于其他账户"
    );
    let actual = File::open(format!("/proc/{}/exe", peer.pid))?;
    let expected = File::open("/proc/self/exe")?;
    ensure!(
        identity(&actual)? == identity(&expected)?,
        "占用者并非同一安装的应用；请手动关闭"
    );
    Ok(peer.pid as u32)
}
#[cfg(target_os = "macos")]
pub(super) fn verify_peer(connection: &Connection) -> Result<u32> {
    unsafe extern "C" {
        fn csops(pid: i32, operation: u32, address: *mut std::ffi::c_void, size: usize) -> i32;
    }
    let fd = connection.0.as_raw_fd();
    let (mut uid, mut gid) = (0, 0);
    ensure!(
        unsafe { libc::getpeereid(fd, &mut uid, &mut gid) } == 0
            && uid == unsafe { libc::geteuid() },
        "无法核实占用者账户"
    );
    let mut pid = 0i32;
    let mut size = size_of::<i32>() as libc::socklen_t;
    // SOL_LOCAL / LOCAL_PEERPID: kernel-attested, live socket peer PID.
    ensure!(
        unsafe { libc::getsockopt(fd, 0, 2, (&mut pid as *mut i32).cast(), &mut size) } == 0
            && size as usize == size_of::<i32>()
            && pid > 0,
        "无法核实占用者进程"
    );
    let mut actual = [0u8; 20];
    let mut expected = [0u8; 20];
    // CS_OPS_CDHASH verifies the loaded image, not a replaceable pathname.
    ensure!(
        unsafe { csops(pid, 5, actual.as_mut_ptr().cast(), actual.len()) } == 0
            && unsafe {
                csops(
                    std::process::id() as i32,
                    5,
                    expected.as_mut_ptr().cast(),
                    expected.len(),
                )
            } == 0
            && actual == expected,
        "无法核实占用者运行代码；请手动关闭"
    );
    Ok(pid as u32)
}
