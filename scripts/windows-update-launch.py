"""CI-only same-user, non-elevated process launch for real updater acceptance."""
import ctypes as c
from ctypes import wintypes as w
import os
from pathlib import Path
import subprocess


class STARTUPINFO(c.Structure):
    _fields_ = [("cb", w.DWORD), ("reserved", w.LPWSTR), ("desktop", w.LPWSTR),
                ("title", w.LPWSTR), ("x", w.DWORD), ("y", w.DWORD), ("width", w.DWORD),
                ("height", w.DWORD), ("xchars", w.DWORD), ("ychars", w.DWORD),
                ("fill", w.DWORD), ("flags", w.DWORD), ("show", w.WORD),
                ("reserved_size", w.WORD), ("reserved_bytes", c.c_void_p),
                ("stdin", w.HANDLE), ("stdout", w.HANDLE), ("stderr", w.HANDLE)]


class PROCESSINFO(c.Structure):
    _fields_ = [("process", w.HANDLE), ("thread", w.HANDLE), ("pid", w.DWORD), ("tid", w.DWORD)]


def apis():
    kernel = c.WinDLL("kernel32", use_last_error=True)
    security = c.WinDLL("advapi32", use_last_error=True)
    kernel.GetCurrentProcess.restype = w.HANDLE
    kernel.CloseHandle.argtypes = [w.HANDLE]
    kernel.OpenProcess.argtypes = [w.DWORD, w.BOOL, w.DWORD]
    kernel.OpenProcess.restype = w.HANDLE
    kernel.WaitForSingleObject.argtypes = [w.HANDLE, w.DWORD]
    kernel.GetExitCodeProcess.argtypes = [w.HANDLE, c.POINTER(w.DWORD)]
    kernel.TerminateProcess.argtypes = [w.HANDLE, w.UINT]
    kernel.QueryFullProcessImageNameW.argtypes = [w.HANDLE, w.DWORD, w.LPWSTR, c.POINTER(w.DWORD)]
    security.OpenProcessToken.argtypes = [w.HANDLE, w.DWORD, c.POINTER(w.HANDLE)]
    security.GetTokenInformation.argtypes = [w.HANDLE, c.c_int, c.c_void_p, w.DWORD, c.POINTER(w.DWORD)]
    security.CreateRestrictedToken.argtypes = [w.HANDLE, w.DWORD, w.DWORD, c.c_void_p,
                                              w.DWORD, c.c_void_p, w.DWORD, c.c_void_p, c.POINTER(w.HANDLE)]
    security.CreateProcessWithTokenW.argtypes = [w.HANDLE, w.DWORD, w.LPCWSTR, w.LPWSTR,
                                                w.DWORD, c.c_void_p, w.LPCWSTR,
                                                c.POINTER(STARTUPINFO), c.POINTER(PROCESSINFO)]
    return kernel, security


def require(value):
    if not value:
        raise c.WinError(c.get_last_error())


def elevated(security, token):
    value, size = w.DWORD(), w.DWORD()
    require(security.GetTokenInformation(token, 20, c.byref(value), c.sizeof(value), c.byref(size)))
    return value.value != 0


def inspect_process(pid):
    kernel, security = apis()
    process = kernel.OpenProcess(0x1000, False, pid)
    require(process)
    token = w.HANDLE()
    try:
        require(security.OpenProcessToken(process, 8, c.byref(token)))
        path, size = c.create_unicode_buffer(32768), w.DWORD(32768)
        require(kernel.QueryFullProcessImageNameW(process, 0, path, c.byref(size)))
        return {"pid": pid, "elevated": elevated(security, token), "executable": path.value}
    finally:
        if token.value:
            kernel.CloseHandle(token)
        kernel.CloseHandle(process)


class Process:
    def __init__(self, handle, pid, kernel):
        self.handle, self.pid, self.kernel = handle, pid, kernel
        self.returncode = None

    def poll(self):
        if self.returncode is None and self.kernel.WaitForSingleObject(self.handle, 0) == 0:
            value = w.DWORD()
            require(self.kernel.GetExitCodeProcess(self.handle, c.byref(value)))
            self.returncode = value.value
        return self.returncode

    def wait(self, timeout):
        if self.kernel.WaitForSingleObject(self.handle, int(timeout * 1000)) != 0:
            raise TimeoutError(f"Process {self.pid} did not exit")
        return self.poll()


def launch(executable, arguments, environment):
    if os.name != "nt" or os.environ.get("GITHUB_ACTIONS") != "true":
        raise RuntimeError("Ordinary-user acceptance launch is restricted to Windows CI")
    kernel, security = apis()
    current, limited = w.HANDLE(), w.HANDLE()
    require(security.OpenProcessToken(kernel.GetCurrentProcess(), 0x000B, c.byref(current)))
    try:
        selected = current
        if elevated(security, current):
            size = w.DWORD()
            # Prefer this same user's existing UAC limited token.
            if not security.GetTokenInformation(current, 19, c.byref(limited), c.sizeof(limited), c.byref(size)):
                # CI images without a linked token can derive a least-privilege token.
                # This only removes authority; production updater authorization is unchanged.
                require(security.CreateRestrictedToken(current, 4, 0, None, 0, None, 0, None, c.byref(limited)))
            selected = limited
        if elevated(security, selected):
            raise RuntimeError("Runner could not supply a non-elevated same-user token")
        executable = str(Path(executable).resolve(strict=True))
        command = c.create_unicode_buffer(subprocess.list2cmdline([executable, *map(str, arguments)]))
        block = c.create_unicode_buffer("\0".join(f"{key}={value}" for key, value in sorted(environment.items())) + "\0\0")
        startup = STARTUPINFO()
        startup.cb = c.sizeof(startup)
        startup.desktop = "winsta0\\default"
        info = PROCESSINFO()
        require(security.CreateProcessWithTokenW(selected, 1, executable, command, 0x400,
                                                 block, str(Path(executable).parent), c.byref(startup), c.byref(info)))
        kernel.CloseHandle(info.thread)
        return Process(info.process, info.pid, kernel)
    finally:
        if limited.value:
            kernel.CloseHandle(limited)
        kernel.CloseHandle(current)
