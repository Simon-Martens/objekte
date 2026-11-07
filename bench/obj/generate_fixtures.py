#!/usr/bin/env python3
"""Create deterministic general-JSON and large-result folders for obj acceptance."""

import argparse
from pathlib import Path


GENERAL = {
    "object.json": b'{"title":"Goethe","year":1800,"values":["text",123,true,null,{"name":"B"},[1,"C"]]}\n',
    "array.json": b'["first",123,true,null,{"title":"A"},[1,"C"]]\n',
    "string.json": b'"a root string"\n',
    "number.json": b'123\n',
    "boolean.json": b'true\n',
    "null.json": b'null\n',
    "empty-array.json": b'[]\n',
    "empty-object.json": b'{}\n',
    "exceptional.json": b'{"large":1234567890123456789012345678901234567890,"tiny":1e-1000000,"huge":1e1000000}\n',
    "escaped.json": b'{"v":"Gr\\u00fcnde","newline":"one\\ntwo","surrogate":"\\ud83d\\ude80"}\n',
    "duplicate.json": b'{"v":"first occurrence","v":"last occurrence"}\n',
    "nested/punctuation.json": b'{"a.b":"punctuation","":42,"@root":"named property"}\n',
    ".hidden/visible.json": b'{"v":"hidden document"}\n',
    ".git/ignored.json": b'{"v":"git metadata"}\n',
    ".objekte/ignored.json": b'{"v":"snapshot metadata"}\n',
    "malformed.json": b'{"v":"unfinished"\n',
}


def write(root: Path, relative: str, data: bytes) -> None:
    path = root / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument("--matches", type=int, default=1024)
    parser.add_argument("--payload-bytes", type=int, default=2048)
    args = parser.parse_args()
    if args.matches < 0 or args.payload_bytes < 0:
        parser.error("matches and payload bytes must be nonnegative")

    for relative, data in GENERAL.items():
        write(args.root / "general", relative, data)
    padding = "x" * args.payload_bytes
    for index in range(args.matches):
        # The first field is deliberately selective and the payload is output-heavy.
        write(
            args.root / "large",
            f"{index:08}.json",
            f'{{"kind":"export-match","ordinal":{index},"padding":"{padding}"}}\n'.encode(),
        )


if __name__ == "__main__":
    main()
