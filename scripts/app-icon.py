"""Convert the approved PNG (--write), or inspect ICO/PE resources without running an app.

Validation uses only Python's standard library. Conversion additionally needs Pillow.
Example: python scripts/app-icon.py --exe target/release/studyapp.exe --installer setup.exe
"""

import argparse
from pathlib import Path
import struct


ROOT = Path(__file__).resolve().parents[1]
SIZES = (16, 24, 32, 48, 64, 128, 256)


def unpack(fmt, data, offset=0):
    return struct.unpack_from("<" + fmt, data, offset)


def read_icon(path):
    data = path.read_bytes()
    reserved, kind, count = unpack("HHH", data)
    if (reserved, kind, count) != (0, 1, len(SIZES)):
        raise ValueError("Expected a Windows icon with seven image sizes")
    frames = []
    for index in range(count):
        width, height, colors, reserved, planes, depth, length, offset = unpack(
            "BBBBHHII", data, 6 + 16 * index
        )
        size = width or 256
        if ((height or 256) != size or size not in SIZES or offset < 6 + count * 16
                or planes != 1 or depth != 32):
            raise ValueError("Invalid icon directory")
        frame = data[offset:offset + length]
        if not length or len(frame) != length:
            raise ValueError("Truncated icon image")
        frames.append(((width, height, colors, reserved, planes, depth), frame))
    if sorted((meta[0] or 256) for meta, _ in frames) != list(SIZES):
        raise ValueError("Missing or duplicate icon sizes")
    return frames


def pe_resources(path):
    """Read the PE resource tree as bytes; never load or execute the file."""
    data = path.read_bytes()
    if data[:2] != b"MZ":
        raise ValueError("Not a PE executable")
    pe, = unpack("I", data, 0x3C)
    if data[pe:pe + 4] != b"PE\0\0":
        raise ValueError("Invalid PE signature")
    sections, = unpack("H", data, pe + 6)
    optional_size, = unpack("H", data, pe + 20)
    optional = pe + 24
    magic, = unpack("H", data, optional)
    if magic not in (0x10B, 0x20B):
        raise ValueError("Unsupported PE optional header")
    directories = optional + (112 if magic == 0x20B else 96)
    resource_rva, resource_size = unpack("II", data, directories + 16)
    if not resource_rva or not resource_size:
        raise ValueError("Executable has no resources")
    section_table = optional + optional_size

    def file_offset(rva):
        for index in range(sections):
            at = section_table + index * 40
            virtual_size, address, raw_size, raw = unpack("IIII", data, at + 8)
            if address <= rva < address + max(virtual_size, raw_size):
                return raw + rva - address
        raise ValueError("Resource address outside PE sections")

    base = file_offset(resource_rva)
    resources = {}

    def walk(relative, keys):
        if len(keys) > 3 or relative + 16 > resource_size:
            raise ValueError("Invalid resource directory")
        named, ids = unpack("HH", data, base + relative + 12)
        for index in range(named + ids):
            at = base + relative + 16 + index * 8
            name, target = unpack("II", data, at)
            key = name
            if name & 0x80000000:
                string_at = base + (name & 0x7FFFFFFF)
                length, = unpack("H", data, string_at)
                key = data[string_at + 2:string_at + 2 + length * 2].decode("utf-16le")
            branch = keys + (key,)
            if target & 0x80000000:
                walk(target & 0x7FFFFFFF, branch)
            else:
                rva, length = unpack("II", data, base + target)
                offset = file_offset(rva)
                payload = data[offset:offset + length]
                if len(branch) != 3 or len(payload) != length:
                    raise ValueError("Invalid resource data")
                resources[branch] = payload

    walk(0, ())
    return resources


def verify_pe(path, expected, require_group_one):
    resources = pe_resources(path)
    matched = []
    for (kind, name, language), group in resources.items():
        if kind != 14 or (require_group_one and name != 1):
            continue
        if unpack("HHH", group) != (0, 1, len(expected)):
            continue
        frames = []
        for index in range(len(expected)):
            entry = unpack("BBBBHHIH", group, 6 + 14 * index)
            payload = resources.get((3, entry[7], language))
            if payload is None or len(payload) != entry[6]:
                break
            frames.append((entry[:6], payload))
        if sorted(frames) == sorted(expected):
            matched.append(name)
    if not matched:
        raise ValueError(f"{path}: approved icon does not match embedded resource group")
    print(f"Verified {path}: matching icon group(s) {matched}, all seven frames exact")


def convert():
    from PIL import Image

    source = ROOT / "assets/icon.png"
    with Image.open(source) as image:
        if image.width != image.height:
            raise ValueError("Approved source must be square; refusing to crop")
        # BMP frames are supported by Windows and NSIS at every requested size.
        image.convert("RGBA").save(
            ROOT / "assets/icon.ico", format="ICO", sizes=[(s, s) for s in SIZES],
            bitmap_format="bmp"
        )
    # Pillow leaves directory planes at zero; use the DIB's single color plane
    # explicitly so Windows resource compilers preserve identical metadata.
    path = ROOT / "assets/icon.ico"
    data = bytearray(path.read_bytes())
    for index in range(len(SIZES)):
        struct.pack_into("<H", data, 6 + index * 16 + 4, 1)
    path.write_bytes(data)
    # Verify decoded pixels against direct source resampling, including alpha.
    with Image.open(source) as original, Image.open(ROOT / "assets/icon.ico") as icon:
        for size in SIZES:
            expected = original.convert("RGBA").resize((size, size), Image.Resampling.LANCZOS)
            if icon.ico.getimage((size, size)).convert("RGBA").tobytes() != expected.tobytes():
                raise ValueError(f"Converted icon differs from approved source at {size}px")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write", action="store_true", help="Regenerate ICO from approved PNG (Pillow required)")
    parser.add_argument("--exe", action="append", type=Path, default=[], help="Verify app icon group 1")
    parser.add_argument("--installer", action="append", type=Path, default=[], help="Verify setup icon group")
    args = parser.parse_args()
    if args.write:
        convert()
    frames = read_icon(ROOT / "assets/icon.ico")
    print("Verified assets/icon.ico: 16, 24, 32, 48, 64, 128 and 256px")
    for path in args.exe:
        verify_pe(path, frames, require_group_one=True)
    for path in args.installer:
        verify_pe(path, frames, require_group_one=False)


if __name__ == "__main__":
    main()
