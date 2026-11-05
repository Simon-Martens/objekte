#!/usr/bin/env python3
"""Download Amazon Reviews'23 and make one Objekte JSON file per review."""

import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import time
import urllib.error
import urllib.request


SOURCE_URL = (
    "https://mcauleylab.ucsd.edu/public_datasets/data/amazon_2023/raw/"
    "review_categories/Musical_Instruments.jsonl.gz"
)
DEFAULT_ROOT = Path(__file__).resolve().parents[2] / "examples" / "amazon-reviews-2023"
ARCHIVE_NAME = "Musical_Instruments.jsonl.gz"


def download(url: str, destination: Path) -> None:
    if destination.is_file():
        print(f"using {destination}", flush=True)
        return
    destination.parent.mkdir(parents=True, exist_ok=True)
    partial = destination.with_name(destination.name + ".part")
    for attempt in range(5):
        position = partial.stat().st_size if partial.exists() else 0
        headers = {"User-Agent": "objekte-benchmark/1.0"}
        if position:
            headers["Range"] = f"bytes={position}-"
        try:
            with urllib.request.urlopen(
                urllib.request.Request(url, headers=headers), timeout=180
            ) as response:
                if position and response.status != 206:
                    # A server that ignores Range must replace the partial file.
                    position = 0
                expected = response.headers.get("Content-Length")
                mode = "ab" if position else "wb"
                with partial.open(mode) as output:
                    while chunk := response.read(1 << 20):
                        output.write(chunk)
                if expected is not None and partial.stat().st_size != position + int(expected):
                    raise OSError("download ended before Content-Length")
            os.replace(partial, destination)
            return
        except (OSError, urllib.error.URLError) as error:
            if attempt == 4:
                raise RuntimeError(f"download failed: {error}") from error
            print(f"download retry {attempt + 1}: {error}", flush=True)
            time.sleep(min(2**attempt, 16))


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(1 << 20):
            digest.update(chunk)
    return digest.hexdigest()


def write_json_file(path: Path, value: dict) -> None:
    if path.exists():
        return
    temporary = path.with_name(path.name + ".part")
    with temporary.open("w", encoding="utf-8") as output:
        json.dump(value, output, ensure_ascii=False, separators=(",", ":"))
        output.write("\n")
    os.replace(temporary, path)


def prepare(archive: Path, root: Path, limit: int, source_url: str) -> None:
    data_dir = root / "data" / "reviews"
    data_dir.mkdir(parents=True, exist_ok=True)
    manifest_path = root / "import-manifest.json"
    archive_digest = sha256(archive)
    if manifest_path.exists():
        previous = json.loads(manifest_path.read_text(encoding="utf-8"))
        if previous.get("archive_sha256") != archive_digest:
            raise ValueError("existing fixture was built from a different archive")
        if previous.get("records", 0) > limit:
            raise ValueError("existing fixture is larger; use a new --root for a smaller limit")
    count = 0
    with gzip.open(archive, "rt", encoding="utf-8") as source:
        for count, line in enumerate(source, start=1):
            if count > limit:
                count = limit
                break
            record = json.loads(line)
            if not isinstance(record, dict):
                raise ValueError(f"review {count} is not a JSON object")
            write_json_file(data_dir / f"{count:07}.json", record)
            if count % 100_000 == 0:
                print(f"prepared {count:,} reviews", flush=True)
    if count < limit:
        raise ValueError(f"archive contains only {count:,} reviews; requested {limit:,}")
    manifest = {
        "source": source_url,
        "archive": archive.name,
        "archive_sha256": archive_digest,
        "collection": "reviews",
        "records": limit,
    }
    temporary = manifest_path.with_name(manifest_path.name + ".part")
    temporary.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    os.replace(temporary, manifest_path)
    print(f"ready: {limit:,} reviews in {data_dir}", flush=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=DEFAULT_ROOT)
    parser.add_argument("--limit", type=int, default=1_000_000)
    parser.add_argument("--archive", type=Path, help="use an already downloaded .jsonl.gz")
    args = parser.parse_args()
    if args.limit < 1:
        parser.error("--limit must be positive")
    archive = args.archive or args.root / "archives" / ARCHIVE_NAME
    if args.archive is None:
        download(SOURCE_URL, archive)
    elif not archive.is_file():
        parser.error(f"archive does not exist: {archive}")
    prepare(archive, args.root, args.limit, SOURCE_URL)


if __name__ == "__main__":
    main()
