//! Process handles for newly created CI fixture images only. No name/PID kill.
use super::*;
use std::{ffi::c_void, os::windows::ffi::OsStringExt};
type NativeHandle = *mut c_void;

#[repr(C)]
struct ProcessEntry {
    size: u32,
    usage: u32,
    pid: u32,
    heap: usize,
    module: u32,
    threads: u32,
    parent: u32,
    priority: i32,
    flags: u32,
    image: [u16; 260],
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> NativeHandle;
    fn Process32FirstW(snapshot: NativeHandle, entry: *mut ProcessEntry) -> i32;
    fn Process32NextW(snapshot: NativeHandle, entry: *mut ProcessEntry) -> i32;
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> NativeHandle;
    fn QueryFullProcessImageNameW(
        process: NativeHandle,
        flags: u32,
        path: *mut u16,
        size: *mut u32,
    ) -> i32;
    fn WaitForSingleObject(handle: NativeHandle, milliseconds: u32) -> u32;
    fn TerminateProcess(process: NativeHandle, code: u32) -> i32;
    fn CloseHandle(handle: NativeHandle) -> i32;
}

struct OwnedHandle(NativeHandle);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

pub(super) struct RunningApplication {
    handle: OwnedHandle,
    pub(super) pid: u32,
}
impl RunningApplication {
    pub(super) fn running(&self) -> Result<bool> {
        match unsafe { WaitForSingleObject(self.handle.0, 0) } {
            0 => Ok(false),
            258 => Ok(true),
            _ => bail!("Cannot inspect fixture child state"),
        }
    }
    pub(super) fn stop(&self) -> Result<()> {
        if self.running()? {
            // The retained handle's full image matched the unique fresh fixture;
            // PID reuse cannot retarget this operation.
            ensure!(
                unsafe { TerminateProcess(self.handle.0, 0) } != 0,
                "Cannot stop the exact CI fixture child"
            );
            ensure!(
                unsafe { WaitForSingleObject(self.handle.0, 10000) } == 0,
                "Fixture child did not exit"
            );
        }
        Ok(())
    }
}
impl Drop for RunningApplication {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

pub(super) fn running_image(expected: &Path) -> Result<RunningApplication> {
    let runner = PathBuf::from(std::env::var_os("RUNNER_TEMP").context("Missing RUNNER_TEMP")?);
    ensure!(
        absolute_components(expected)?.starts_with(&absolute_components(&runner)?),
        "Fixture process image must belong to RUNNER_TEMP"
    );
    let snapshot = unsafe { CreateToolhelp32Snapshot(2, 0) };
    ensure!(
        !snapshot.is_null() && snapshot as isize != -1,
        "Cannot inspect CI fixture process"
    );
    let snapshot = OwnedHandle(snapshot);
    // SAFETY: zero initializes native storage; size is set before the writable
    // PROCESSENTRY32W is passed to the enumeration APIs.
    let mut entry: ProcessEntry = unsafe { std::mem::zeroed() };
    entry.size = std::mem::size_of::<ProcessEntry>() as u32;
    let mut next = unsafe { Process32FirstW(snapshot.0, &mut entry) };
    let mut matches = Vec::new();
    while next != 0 {
        let handle = unsafe { OpenProcess(0x0010_1001, 0, entry.pid) };
        if !handle.is_null() {
            let handle = OwnedHandle(handle);
            let mut path = [0u16; 32768];
            let mut length = path.len() as u32;
            if unsafe { QueryFullProcessImageNameW(handle.0, 0, path.as_mut_ptr(), &mut length) }
                != 0
                && length < path.len() as u32
            {
                let image = PathBuf::from(std::ffi::OsString::from_wide(&path[..length as usize]));
                if same_absolute_path(&image, expected).unwrap_or(false) {
                    matches.push((handle, entry.pid));
                }
            }
        }
        next = unsafe { Process32NextW(snapshot.0, &mut entry) };
    }
    ensure!(
        matches.len() == 1,
        "Expected exactly one real process for the fresh fixture image"
    );
    let (handle, pid) = matches.pop().unwrap();
    let process = RunningApplication { handle, pid };
    ensure!(process.running()?, "Storage-ready child exited early");
    Ok(process)
}
