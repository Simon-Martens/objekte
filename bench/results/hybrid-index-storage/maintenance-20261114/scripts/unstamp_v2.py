#!/usr/bin/env python3
"""Clear v2 validation stamps in a disposable benchmark store."""

import hashlib
import os
from pathlib import Path
import struct
import sys


def clear(path: Path) -> int:
    data = bytearray(path.read_bytes())
    if len(data) > 4 * 1024 * 1024 or data[:8] != b"OBJHMAN1":
        raise ValueError(f"unsupported manifest: {path}")
    if hashlib.sha256(data[:-32]).digest() != data[-32:]:
        raise ValueError(f"manifest checksum mismatch: {path}")
    if struct.unpack_from("<II", data, 8) != (2, 1):
        raise ValueError(f"manifest is not v2/segment-v1: {path}")
    cursor = 20 + 32 + 32

    def read(fmt: str) -> int:
        nonlocal cursor
        size = struct.calcsize(fmt)
        if cursor + size > len(data) - 32:
            raise ValueError("truncated manifest")
        value = struct.unpack_from(fmt, data, cursor)[0]
        cursor += size
        return value

    root_length = read("<I")
    cursor += root_length  # canonical root bytes
    read("<Q")  # generation
    marker = read("<B")
    if marker == 1:
        name_length = read("<H")
        cursor += name_length  # catalog name
        cursor += 8 + 32  # length and SHA-256
    elif marker != 0:
        raise ValueError("catalog marker")
    count = read("<I")
    if count > 16_384:
        raise ValueError("segment count")
    changed = 0
    for _ in range(count):
        name_length = read("<H")
        cursor += name_length  # segment name
        cursor += 8 + 4 + 8  # length, collection, ordinal
        if read("<I") != 0:
            struct.pack_into("<I", data, cursor - 4, 0)
            changed += 1
        cursor += 4 + 4  # slot count and live count
        mask = read("<B")
        if mask == 1:
            mask_length = read("<I")
            cursor += mask_length
        elif mask != 0:
            raise ValueError("active mask kind")
    if cursor != len(data) - 32:
        raise ValueError("manifest trailing bytes")
    data[-32:] = hashlib.sha256(data[:-32]).digest()
    temporary = path.with_suffix(".unstamp-tmp")
    with temporary.open("xb") as output:
        os.fchmod(output.fileno(), 0o600)
        output.write(data)
        output.flush()
        os.fsync(output.fileno())
    os.replace(temporary, path)
    directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)
    return changed


if __name__ == "__main__":
    for name in sys.argv[1:]:
        path = Path(name)
        print(f"{path}: {clear(path)} stamps cleared")
