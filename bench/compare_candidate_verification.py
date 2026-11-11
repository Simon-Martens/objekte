"""Check focused benchmark equivalence and summarize timing ratios."""

import argparse
import json
from pathlib import Path


def read(path: Path) -> tuple[dict, dict[tuple[str, str], dict]]:
    rows = [json.loads(line) for line in path.read_text().splitlines() if line]
    setup = next((row for row in rows if row.get("level") == "setup"), None)
    if setup is None:
        raise ValueError(f"{path}: missing setup row")
    cases = {
        (row["case"], row["level"]): row
        for row in rows
        if row.get("case") is not None
    }
    if len(cases) != len(rows) - 1:
        raise ValueError(f"{path}: duplicate case/level row")
    return setup, cases


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("baseline", type=Path)
    parser.add_argument("variant", type=Path)
    args = parser.parse_args()
    base_setup, base_cases = read(args.baseline)
    variant_setup, variant_cases = read(args.variant)

    for field in ("corpus_sha256", "records", "source_bytes", "workers", "verified_cache_bytes"):
        if field not in base_setup or field not in variant_setup:
            raise ValueError(f"setup is missing {field}")
        if base_setup[field] != variant_setup[field]:
            raise ValueError(f"setup differs for {field}: {base_setup[field]} vs {variant_setup[field]}")
    if not base_cases.keys() <= variant_cases.keys():
        raise ValueError("variant is missing a baseline case/level")
    extra = variant_cases.keys() - base_cases.keys()
    if any(level != "in_memory_trusted_parse_extract_evaluate" for _, level in extra):
        raise ValueError("variant has unexpected case/level rows")

    print("case\tlevel\tbase_ms\tvariant_ms\tratio\tbase_alloc_bytes\tvariant_alloc_bytes")
    for key in sorted(base_cases):
        base = base_cases[key]
        variant = variant_cases[key]
        for field in ("matches", "member_sha256"):
            if base[field] != variant[field]:
                raise ValueError(f"{key}: {field} differs")
        if base["level"] == "warm_repository_search":
            for field in ("candidates", "scanned_records", "bytes_read"):
                if base["counters"][field] != variant["counters"][field]:
                    raise ValueError(f"{key}: work counter {field} differs")
        base_ms = base["median_ns"] / 1_000_000
        variant_ms = variant["median_ns"] / 1_000_000
        print(
            f"{key[0]}\t{key[1]}\t{base_ms:.3f}\t{variant_ms:.3f}\t"
            f"{variant_ms / base_ms:.3f}\t{base.get('allocated_bytes', '-')}\t"
            f"{variant.get('allocated_bytes', '-')}"
        )
    for case, level in sorted(extra):
        base = base_cases.get((case, "in_memory_validating_parse_extract_evaluate"))
        variant = variant_cases[(case, level)]
        if base is None or any(base[field] != variant[field] for field in ("matches", "member_sha256")):
            raise ValueError(f"{case}: trusted membership differs from validating baseline")
        base_ms = base["median_ns"] / 1_000_000
        variant_ms = variant["median_ns"] / 1_000_000
        print(
            f"{case}\ttrusted_vs_base_validating\t{base_ms:.3f}\t{variant_ms:.3f}\t"
            f"{variant_ms / base_ms:.3f}\t{base.get('allocated_bytes', '-')}\t"
            f"{variant.get('allocated_bytes', '-')}"
        )


if __name__ == "__main__":
    main()
