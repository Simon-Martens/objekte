import json
import sys
from pathlib import Path


root = Path(__file__).parent
baseline = json.loads((root / "baseline.json").read_text())
candidate = json.loads((root / "candidate.json").read_text())

expected = {
    "enabled = true": list(range(0, 32, 2)),
    "group = 'A'": list(range(0, 32, 2)),
    "sequence = 31": [31],
}
expected_diagnostics = {
    "enabled": ("Built", 2, 32),
    "group": ("Built", 2, 32),
    "sequence": ("Built", 32, 32),
}


def check_run(result, revision):
    assert result["revision"] == revision
    assert result["fixture_sha256"] == (
        "9c76b850fca687ebfc24758b164f178de5237b7e93009f00f1054d65cc80fcbb"
    )
    diagnostics = {tuple(item["path"]): item for item in result["diagnostics"]}
    for path, (status, value_keys, covered_records) in expected_diagnostics.items():
        item = diagnostics[(path,)]
        assert item["status"] == status
        assert item["value_keys"] == value_keys
        assert item["covered_records"] == covered_records
        assert item["value_dictionary_complete"] is True

    queries = {item["filter"]: item for item in result["queries"]}
    assert set(queries) == set(expected)
    for filter, rows in expected.items():
        item = queries[filter]
        assert item["selected"] == {"kind": "posting", "exact": True}
        assert item["count"] == len(rows)
        assert item["page_len"] == len(rows)
        assert item["counters"] == {
            "files_opened": 0,
            "record_parses": 0,
            "scanned_records": 0,
            "residual_evaluations": 0,
        }
        observed = [locator["row"] for locator in item["locators"]]
        assert len(observed) == len(set(observed))
        assert set(observed) == set(rows)
        assert all(locator["collection"] == "records" for locator in item["locators"])
        assert all(locator["file_id"] == result["file_id"] for locator in item["locators"])


check_run(baseline, "baseline")
check_run(candidate, "candidate")
assert baseline["queries"] == candidate["queries"]

summary = {
    "baseline_sha": baseline["tested_sha"],
    "candidate_sha": candidate["tested_sha"],
    "fixture_sha256": candidate["fixture_sha256"],
    "ordered_locators_equal": True,
    "query_count": len(baseline["queries"]) + len(candidate["queries"]),
    "queries": [
        {
            "revision": revision["revision"],
            "filter": query["filter"],
            "selected": query["selected"],
            "count": query["count"],
            "rows": [locator["row"] for locator in query["locators"]],
            "counters": query["counters"],
        }
        for revision in (baseline, candidate)
        for query in revision["queries"]
    ],
}
(root / "comparison.json").write_text(json.dumps(summary, separators=(",", ":")) + "\n")
print(json.dumps(summary, indent=2))
