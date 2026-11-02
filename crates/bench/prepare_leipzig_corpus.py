#!/usr/bin/env python3
"""Build a repeatable multilingual JSON corpus from Leipzig sentence archives."""

import argparse
import hashlib
import io
import json
from pathlib import Path
import tarfile
import urllib.request


LANGUAGES = ("ara", "deu", "fra", "jpn", "pol", "rus", "spa", "zho")
ARCHIVE_SUFFIX = "_news_2020_100K.tar.gz"
BASE_URL = "https://downloads.wortschatz-leipzig.de/corpora/"
SHA256 = {
    "ara": "ad4eaa54154c4cf4e9ec0f5da42e0528fcd35badcc40fd31f6c0c39a68f3f37d",
    "deu": "20d96f9e3686ff236b9e40e5d92fca8369479aa4eec01397d16a9036bac765c6",
    "fra": "0013db37d9081956e910769a29901bd3a3c34fca3051fc2839cd9b85e73ea133",
    "jpn": "5fbaa0f02f0d9bebcb60b3a481e2c49e01d08b136bae5f77496273a4b7fbc683",
    "pol": "156572f69e8cad38901679010f59ca596784feca86c97940febba9f8538e869a",
    "rus": "2f78e5a68873ee586b29a36f6daa45ec8ab90edda6defc9b2911a4857b211b7",
    "spa": "96323c33f61720dce3f15d41c7e3d375d19de2acd1eebf106cb28dc476360217",
    "zho": "ba219ac3a5b9e13f2653690ab67a03a184cab6019cbbb374791fd4cc7446a7e1",
}


def archive_for(code: str, archive_dir: Path) -> Path:
    name = code + ARCHIVE_SUFFIX
    path = archive_dir / name
    if not path.exists():
        print(f"downloading {name}", flush=True)
        with urllib.request.urlopen(BASE_URL + name, timeout=180) as source:
            with path.open("wb") as destination:
                while chunk := source.read(1 << 20):
                    destination.write(chunk)
    return path


def write_language(code: str, archive: Path, data_dir: Path, limit: int) -> int:
    stem = archive.name.removesuffix(".tar.gz")
    member_name = f"{stem}/{stem}-sentences.txt"
    count = 0
    with tarfile.open(archive, "r:gz") as bundle:
        source = bundle.extractfile(member_name)
        if source is None:
            raise ValueError(f"missing {member_name}")
        for raw_line in io.TextIOWrapper(source, encoding="utf-8"):
            number, separator, sentence = raw_line.rstrip("\n").partition("\t")
            if not separator or not sentence.strip():
                continue
            record = json.dumps({"text": sentence}, ensure_ascii=False, separators=(",", ":"))
            (data_dir / f"{code}_{int(number):06}.json").write_text(record, encoding="utf-8")
            count += 1
            if count == limit:
                break
    if count != limit:
        raise ValueError(f"{code}: expected {limit} sentences, got {count}")
    return count


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument("--per-language", type=int, default=50_000)
    args = parser.parse_args()
    if not 1 <= args.per_language <= 100_000:
        parser.error("--per-language must be 1–100000")
    archive_dir = args.root / "archives"
    data_dir = args.root / "data" / "texts"
    archive_dir.mkdir(parents=True, exist_ok=True)
    data_dir.mkdir(parents=True, exist_ok=True)
    manifest = {}
    for code in LANGUAGES:
        archive = archive_for(code, archive_dir)
        with archive.open("rb") as source:
            digest = hashlib.file_digest(source, "sha256").hexdigest()
        if digest != SHA256[code]:
            raise ValueError(f"{archive}: SHA-256 changed: {digest}")
        count = write_language(code, archive, data_dir, args.per_language)
        manifest[code] = {"archive": archive.name, "sha256": digest, "records": count}
        print(f"{code}: {count} records", flush=True)
    (archive_dir / "corpus-manifest.json").write_text(
        json.dumps(manifest, indent=2) + "\n", encoding="utf-8"
    )


if __name__ == "__main__":
    main()
