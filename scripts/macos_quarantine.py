"""Read existing Darwin quarantine metadata without modifying it."""
import ctypes
import os


def read_quarantine(path):
    library = ctypes.CDLL("/usr/lib/libSystem.B.dylib", use_errno=True)
    getter = library.getxattr
    getter.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_void_p,
                       ctypes.c_size_t, ctypes.c_uint32, ctypes.c_int]
    getter.restype = ctypes.c_ssize_t
    buffer = ctypes.create_string_buffer(65536)
    length = getter(os.fsencode(path), b"com.apple.quarantine", buffer, len(buffer), 0, 1)
    if length < 0:
        error = ctypes.get_errno()
        if error == 93:  # Darwin ENOATTR; other failures are not absence.
            return None
        raise OSError(error, "Cannot read existing quarantine metadata", str(path))
    return buffer.raw[:length].hex()
