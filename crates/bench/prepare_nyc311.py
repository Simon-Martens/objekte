#!/usr/bin/env python3
"""Download NYC 311 requests and make one Objekte JSON file per request."""

import argparse
import gzip
import json
import os
from pathlib import Path
import re
import time
import urllib.error
import urllib.parse
import urllib.request


SOURCE_URL = "https://data.cityofnewyork.us/resource/erm2-nwe9.json"
DEFAULT_ROOT = Path(__file__).resolve().parents[2] / "examples" / "nyc311"
KEY_RE = re.compile(r"[0-9]+")


def fetch_page(url: str, last_key: str | None, page_size: int) -> list[dict]:
    query = {"$limit": str(page_size), "$order": "unique_key ASC"}
    if last_key is not None:
        query["$where"] = f"unique_key > '{last_key}'"
    request_url = url + "?" + urllib.parse.urlencode(query)
    request = urllib.request.Request(
        request_url,
        headers={"User-Agent": "objekte-benchmark/1.0", "Accept-Encoding": "gzip"},
    )
    for attempt in range(6):
        try:
            with urllib.request.urlopen(request, timeout=180) as response:
                if response.headers.get("Content-Encoding", "").lower() == "gzip":
                    payload = gzip.decompress(response.read())
                else:
                    payload = response.read()
            records = json.loads(payload)
            if not isinstance(records, list):
                raise ValueError(f"API returned an error instead of a page: {records}")
            return records
        except (OSError, urllib.error.URLError) as error:
            if attempt == 5:
                raise RuntimeError(f"request failed: {error}") from error
            print(f"request retry {attempt + 1}: {error}", flush=True)
            time.sleep(min(2**attempt, 30))
    raise AssertionError("unreachable")


def atomic_json(path: Path, value: dict) -> None:
    temporary = path.with_name(path.name + ".part")
    with temporary.open("w", encoding="utf-8") as output:
        json.dump(value, output, ensure_ascii=False, separators=(",", ":"))
        output.write("\n")
    os.replace(temporary, path)


def prepare(root: Path, limit: int, page_size: int, source_url: str) -> None:
    data_dir = root / "data" / "requests"
    data_dir.mkdir(parents=True, exist_ok=True)
    manifest_path = root / "import-manifest.json"
    if manifest_path.exists():
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        if manifest.get("source") != source_url:
            raise ValueError("existing manifest belongs to a different source")
        count = manifest["records"]
        last_key = manifest["last_key"]
        if count > limit:
            raise ValueError("existing fixture is larger; use a new --root for a smaller limit")
    else:
        count = 0
        last_key = None
    while count < limit:
        page = fetch_page(source_url, last_key, min(page_size, limit - count))
        if not page:
            raise ValueError(f"source ended after {count:,} requests; requested {limit:,}")
        previous_key = last_key
        for record in page:
            if not isinstance(record, dict):
                raise ValueError(f"API returned a non-object record: {record!r}")
            key = record.get("unique_key")
            if not isinstance(key, str) or not KEY_RE.fullmatch(key):
                raise ValueError(f"invalid unique_key: {key!r}")
            if previous_key is not None and key <= previous_key:
                raise ValueError(f"out-of-order or duplicate unique_key: {key}")
            atomic_json(data_dir / f"{key}.json", record)
            previous_key = key
        count += len(page)
        last_key = previous_key
        atomic_json(
            manifest_path,
            {"source": source_url, "collection": "requests", "records": count,
             "last_key": last_key, "page_size": page_size},
        )
        print(f"prepared {count:,} requests (last key {last_key})", flush=True)
    print(f"ready: {count:,} requests in {data_dir}", flush=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=DEFAULT_ROOT)
    parser.add_argument("--limit", type=int, default=1_000_000)
    parser.add_argument("--page-size", type=int, default=10_000)
    parser.add_argument("--source-url", default=SOURCE_URL, help="Socrata JSON endpoint")
    args = parser.parse_args()
    if args.limit < 1 or not 1 <= args.page_size <= 50_000:
        parser.error("limit must be positive and page-size must be 1–50000")
    prepare(args.root, args.limit, args.page_size, args.source_url)


if __name__ == "__main__":
    main()
