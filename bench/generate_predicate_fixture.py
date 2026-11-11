#!/usr/bin/env python3
"""Write the deterministic local predicate benchmark corpus."""
import argparse
import hashlib
import json
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("root", type=Path)
args = parser.parse_args()
data = args.root / "data"
if data.exists() and any(data.iterdir()):
    parser.error(f"refusing to overwrite a populated fixture: {data}")
data.mkdir(parents=True, exist_ok=True)
digest = hashlib.sha256()
for slot in range(1000):
    kind = "hot" if slot < 950 else "cold"
    if slot in (949, 970):
        body = "needle"
    elif slot % 100 == 0:
        body = "Straße with escaped \\ and ünicode"
    else:
        body = "ordinary"
    name = f"{slot:04}.json"
    tag = f"{kind}{slot:04}"
    memo = "needle" if slot % 100 == 0 else "ordinary"
    content = json.dumps({"kind": kind, "body": body, "tag": tag, "memo": memo}, ensure_ascii=False, separators=(",", ":"))
    encoded = content.encode("utf-8")
    (data / name).write_bytes(encoded)
    digest.update(name.encode("ascii") + b"\0" + encoded + b"\0")
manifest = {"records": 1000, "sha256": digest.hexdigest(), "generator": "bench/generate_predicate_fixture.py"}
(args.root / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
print(json.dumps(manifest))
