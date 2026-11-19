import json
from pathlib import Path


root = Path(__file__).parent
a1 = json.loads((root / "a1-reads.json").read_text())
candidate = json.loads((root / "candidate-reads.json").read_text())

assert a1["tested_sha"] == "b4172a1749750ee1c87ef74a56f5ee7cec6c40ee"
assert candidate["tested_sha"] == "c34294489f0011ca7a0009b54e0af5a6d6b600b3"
assert a1["results"] == candidate["results"]

expected = {
    "exact_page": {
        "count": 2,
        "ids": ["path:00.json"],
        "files_opened": 1,
        "distinct_files": 1,
        "bytes_read": 0,
        "record_parses": 1,
        "candidates": 2,
        "scanned_records": 0,
        "residual_evaluations": 0,
    },
    "combined_old": {
        "count": 3,
        "ids": ["path:00.json", "path:01.json", "path:03.json"],
        "files_opened": 3,
        "distinct_files": 6,
        "bytes_read": 165,
        "record_parses": 3,
        "candidates": 3,
        "scanned_records": 3,
        "residual_evaluations": 3,
    },
    "combined_number": {
        "count": 2,
        "ids": ["path:00.json", "path:01.json"],
        "files_opened": 3,
        "distinct_files": 5,
        "bytes_read": 167,
        "record_parses": 3,
        "candidates": 3,
        "scanned_records": 3,
        "residual_evaluations": 3,
    },
}

for revision in (a1, candidate):
    for result in revision["results"]:
        counters = result["counters"]
        expected_counters = expected[result["name"]]
        assert result["count"] == expected_counters["count"]
        assert result["ids"] == expected_counters["ids"]
        for key in (
            "files_opened",
            "distinct_files",
            "bytes_read",
            "record_parses",
            "candidates",
            "scanned_records",
            "residual_evaluations",
        ):
            assert counters[key] == expected_counters[key]

comparison = {
    "a1_sha": a1["tested_sha"],
    "candidate_sha": candidate["tested_sha"],
    "results_equal": True,
    "results": a1["results"],
}
(root / "comparison.json").write_text(
    json.dumps(comparison, separators=(",", ":")) + "\n"
)
print(json.dumps(comparison, indent=2))
