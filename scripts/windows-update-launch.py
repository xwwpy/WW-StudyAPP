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
    kernel.LocalFree.argtypes = [c.c_void_p]
    kernel.LocalFree.restype = c.c_void_p
    kernel.OpenProcess.argtypes = [w.DWORD, w.BOOL, w.DWORD]
    kernel.OpenProcess.restype = w.HANDLE
    kernel.WaitForSingleObject.argtypes = [w.HANDLE, w.DWORD]
    kernel.GetExitCodeProcess.argtypes = [w.HANDLE, c.POINTER(w.DWORD)]
    kernel.TerminateProcess.argtypes = [w.HANDLE, w.UINT]
    kernel.QueryFullProcessImageNameW.argtypes = [w.HANDLE, w.DWORD, w.LPWSTR, c.POINTER(w.DWORD)]
    security.OpenProcessToken.argtypes = [w.HANDLE, w.DWORD, c.POINTER(w.HANDLE)]
    security.GetTokenInformation.argtypes = [w.HANDLE, c.c_int, c.c_void_p, w.DWORD, c.POINTER(w.DWORD)]
    security.ConvertSidToStringSidW.argtypes = [c.c_void_p, c.POINTER(w.LPWSTR)]
    security.ConvertStringSidToSidW.argtypes = [w.LPCWSTR, c.POINTER(c.c_void_p)]
    security.GetLengthSid.argtypes = [c.c_void_p]
    security.GetLengthSid.restype = w.DWORD
    security.SetTokenInformation.argtypes = [w.HANDLE, c.c_int, c.c_void_p, w.DWORD]
    security.CreateRestrictedToken.argtypes = [w.HANDLE, w.DWORD, w.DWORD, c.c_void_p,
                                              w.DWORD, c.c_void_p, w.DWORD, c.c_void_p, c.POINTER(w.HANDLE)]
    security.CreateProcessWithTokenW.argtypes = [w.HANDLE, w.DWORD, w.LPCWSTR, w.LPWSTR,
                                                w.DWORD, c.c_void_p, w.LPCWSTR,
                                                c.POINTER(STARTUPINFO), c.POINTER(PROCESSINFO)]
    security.CreateProcessAsUserW.argtypes = [w.HANDLE, w.LPCWSTR, w.LPWSTR, c.c_void_p, c.c_void_p,
                                             w.BOOL, w.DWORD, c.c_void_p, w.LPCWSTR,
                                             c.POINTER(STARTUPINFO), c.POINTER(PROCESSINFO)]
    return kernel, security


def require(value):
    if not value:
        raise c.WinError(c.get_last_error())


def elevated(security, token):
    value, size = w.DWORD(), w.DWORD()
    require(security.GetTokenInformation(token, 20, c.byref(value), c.sizeof(value), c.byref(size)))
    return value.value != 0


def token_sid(kernel, security, token, information_class=1):
    size = w.DWORD()
    security.GetTokenInformation(token, information_class, None, 0, c.byref(size))
    if not 0 < size.value < 65536:
        raise RuntimeError("Invalid TokenUser buffer size")
    buffer = c.create_string_buffer(size.value)
    require(security.GetTokenInformation(token, information_class, buffer, size, c.byref(size)))
    sid = c.cast(buffer, c.POINTER(c.c_void_p))[0]
    text = w.LPWSTR()
    require(security.ConvertSidToStringSidW(sid, c.byref(text)))
    try:
        return text.value
    finally:
        kernel.LocalFree(c.cast(text, c.c_void_p))


def inspect_process(pid):
    kernel, security = apis()
    process = kernel.OpenProcess(0x1000, False, pid)
    require(process)
    token = w.HANDLE()
    try:
        require(security.OpenProcessToken(process, 8, c.byref(token)))
        path, size = c.create_unicode_buffer(32768), w.DWORD(32768)
        require(kernel.QueryFullProcessImageNameW(process, 0, path, c.byref(size)))
        return {"pid": pid, "elevated": elevated(security, token), "executable": path.value,
                "user_sid": token_sid(kernel, security, token), "integrity_sid": token_sid(kernel, security, token, 25)}
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


def allow_same_user_desktop(kernel, security, sid_text):
    """Grant only this runner user access to its existing interactive desktop."""
    user = c.WinDLL("user32", use_last_error=True)
    user.GetProcessWindowStation.restype = w.HANDLE
    user.GetThreadDesktop.argtypes = [w.DWORD]
    user.GetThreadDesktop.restype = w.HANDLE
    class Trustee(c.Structure):
        _fields_ = [("multiple", c.c_void_p), ("operation", c.c_int), ("form", c.c_int), ("kind", c.c_int), ("name", c.c_void_p)]
    class Access(c.Structure):
        _fields_ = [("permissions", w.DWORD), ("mode", c.c_int), ("inheritance", w.DWORD), ("trustee", Trustee)]
    security.GetSecurityInfo.argtypes = [w.HANDLE, c.c_int, w.DWORD, c.c_void_p, c.c_void_p, c.POINTER(c.c_void_p), c.c_void_p, c.POINTER(c.c_void_p)]
    security.SetEntriesInAclW.argtypes = [w.ULONG, c.POINTER(Access), c.c_void_p, c.POINTER(c.c_void_p)]
    security.SetSecurityInfo.argtypes = [w.HANDLE, c.c_int, w.DWORD, c.c_void_p, c.c_void_p, c.c_void_p, c.c_void_p]
    sid = c.c_void_p()
    require(security.ConvertStringSidToSidW(sid_text, c.byref(sid)))
    try:
        for handle, rights in [(user.GetProcessWindowStation(), 0x000F037F), (user.GetThreadDesktop(kernel.GetCurrentThreadId()), 0x000F01FF)]:
            require(handle)
            old, descriptor, new = c.c_void_p(), c.c_void_p(), c.c_void_p()
            try:
                error = security.GetSecurityInfo(handle, 7, 4, None, None, c.byref(old), None, c.byref(descriptor))
                if error:
                    raise c.WinError(error)
                entry = Access(rights, 1, 0, Trustee(None, 0, 0, 1, sid))
                error = security.SetEntriesInAclW(1, c.byref(entry), old, c.byref(new))
                if error:
                    raise c.WinError(error)
                error = security.SetSecurityInfo(handle, 7, 4, None, None, new, None)
                if error:
                    raise c.WinError(error)
            finally:
                if new.value:
                    kernel.LocalFree(new)
                if descriptor.value:
                    kernel.LocalFree(descriptor)
    finally:
        kernel.LocalFree(sid)


def launch(executable, arguments, environment, headless=False):
    if os.name != "nt" or os.environ.get("GITHUB_ACTIONS") != "true":
        raise RuntimeError("Ordinary-user acceptance launch is restricted to Windows CI")
    kernel, security = apis()
    current, limited = w.HANDLE(), w.HANDLE()
    require(security.OpenProcessToken(kernel.GetCurrentProcess(), 0x008B, c.byref(current)))
    try:
        selected = current
        restricted = False
        if not elevated(security, current):
            return subprocess.Popen([str(executable), *map(str, arguments)], env=environment,
                                    creationflags=0x08000000 if headless else 0)
        else:
            size = w.DWORD()
            # Prefer this same user's existing UAC limited token.
            if not security.GetTokenInformation(current, 19, c.byref(limited), c.sizeof(limited), c.byref(size)):
                # CI images without a linked token can derive a least-privilege token.
                # This only removes authority; production updater authorization is unchanged.
                require(security.CreateRestrictedToken(current, 4, 0, None, 0, None, 0, None, c.byref(limited)))
                restricted = True
            selected = limited
        if elevated(security, selected):
            raise RuntimeError("Runner could not supply a non-elevated same-user token")
        if token_sid(kernel, security, selected) != token_sid(kernel, security, current):
            raise RuntimeError("Acceptance launch token changed the user identity")
        integrity = token_sid(kernel, security, selected, 25)
        if restricted and int(integrity.rsplit("-", 1)[1]) > 8192:
            # The runner's built-in Administrator has no linked medium token.
            # Lower only our newly derived restricted token, never the caller token.
            class Label(c.Structure):
                _fields_ = [("sid", c.c_void_p), ("attributes", w.DWORD)]
            sid = c.c_void_p()
            require(security.ConvertStringSidToSidW("S-1-16-8192", c.byref(sid)))
            try:
                label = Label(sid, 0x20)
                require(security.SetTokenInformation(selected, 25, c.byref(label), c.sizeof(label) + security.GetLengthSid(sid)))
            finally:
                kernel.LocalFree(sid)
            integrity = token_sid(kernel, security, selected, 25)
        if int(integrity.rsplit("-", 1)[1]) > 8192:
            raise RuntimeError(f"Acceptance token integrity is above ordinary user: {integrity}")
        if restricted:
            allow_same_user_desktop(kernel, security, token_sid(kernel, security, selected))
        executable = str(Path(executable).resolve(strict=True))
        command = c.create_unicode_buffer(subprocess.list2cmdline([executable, *map(str, arguments)]))
        block = c.create_unicode_buffer("\0".join(f"{key}={value}" for key, value in sorted(environment.items())) + "\0\0")
        startup = STARTUPINFO()
        startup.cb = c.sizeof(startup)
        startup.desktop = "winsta0\\default"
        info = PROCESSINFO()
        flags = 0x400 | (0x08000000 if headless else 0)
        if restricted:
            created = security.CreateProcessAsUserW(selected, executable, command, None, None, False,
                                                     flags, block, str(Path(executable).parent), c.byref(startup), c.byref(info))
        else:
            # This user's profile is already loaded; do not request another profile logon.
            created = security.CreateProcessWithTokenW(selected, 0, executable, command, flags,
                                                        block, str(Path(executable).parent), c.byref(startup), c.byref(info))
        if not created:
            error = c.WinError(c.get_last_error())
            raise RuntimeError(f"Ordinary launch failed ({'restricted/current-user' if restricted else 'linked-user'}; integrity={integrity}): {error}")
        kernel.CloseHandle(info.thread)
        return Process(info.process, info.pid, kernel)
    finally:
        if limited.value:
            kernel.CloseHandle(limited)
        kernel.CloseHandle(current)
