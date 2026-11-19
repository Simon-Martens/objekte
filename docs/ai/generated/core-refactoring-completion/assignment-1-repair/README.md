# Assignment 1 inference-posting verification

This evidence compares the original series base B with repair source C and
shows that optimized queries consume the inferred posting dictionaries through
the published `IndexView` interface.

| Revision | SHA |
|---|---|
| Original series base B | `0f1d5e5ffa566a911f926ffe564e49c0c69ded77` |
| Repair source C | `9a234188f82e456f1e245fae07b179b64878b05a` |

The fixture SHA-256 is
`9c76b850fca687ebfc24758b164f178de5237b7e93009f00f1054d65cc80fcbb`.
Its size is 2,534 bytes, modification time is
`2026-11-19 14:40:59.055909321 +0100`, and mtime seconds are `1795095659`.
Both probe runs used `/tmp/objekte-assignment1-repair-fixture.H4Tt9W` without
rewriting `data/records.jsonl`.

## Reproduction

Create the fixed fixture once:

```sh
fixture_root=$(mktemp -d)
python3 fixture.py "$fixture_root"
sha256sum "$fixture_root/data/records.jsonl"
```

In a detached baseline worktree at B, copy `probe-b.rs` to
`crates/core/examples/assignment_1_inference_probe.rs`, then run:

```sh
cargo run --quiet --locked -p objekte-search-core \
  --example assignment_1_inference_probe -- \
  --root "$fixture_root" --revision baseline \
  --tested-sha 0f1d5e5ffa566a911f926ffe564e49c0c69ded77 \
  --fixture-sha256 9c76b850fca687ebfc24758b164f178de5237b7e93009f00f1054d65cc80fcbb \
  > baseline.json
```

At C, copy `probe-c.rs` to the same example path and run:

```sh
cargo run --quiet --locked -p objekte-search-core \
  --example assignment_1_inference_probe -- \
  --root "$fixture_root" --revision candidate \
  --tested-sha 9a234188f82e456f1e245fae07b179b64878b05a \
  --fixture-sha256 9c76b850fca687ebfc24758b164f178de5237b7e93009f00f1054d65cc80fcbb \
  > candidate.json
```

The only source adaptation between the probes is the `ReturnMode` import:
`execution_options` at B and `query::options` at C. Validate both saved runs:

```sh
python3 verify.py
```

## Results

`verify.py` exits unsuccessfully if either revision differs from the expected
count, page membership, plan, counters, diagnostics, or if the ordered locator
sequence differs between revisions. The six observed query results are:

| Revision | Filter | Count | Rows | Plan | Files opened | Records parsed |
|---|---|---:|---|---|---:|---:|
| baseline | `enabled = true` | 16 | even rows 0…30 | exact posting | 0 | 0 |
| baseline | `group = 'A'` | 16 | even rows 0…30 | exact posting | 0 | 0 |
| baseline | `sequence = 31` | 1 | 31 | exact posting | 0 | 0 |
| candidate | `enabled = true` | 16 | even rows 0…30 | exact posting | 0 | 0 |
| candidate | `group = 'A'` | 16 | even rows 0…30 | exact posting | 0 | 0 |
| candidate | `sequence = 31` | 1 | 31 | exact posting | 0 | 0 |

All runs also recorded zero scanned records and zero residual evaluations.
The diagnostic value-key counts and 32-record coverage were identical:

| Path | Status | Value keys | Covered records | Complete |
|---|---|---:|---:|---|
| `enabled` | Built | 2 | 32 | yes |
| `group` | Built | 2 | 32 | yes |
| `sequence` | Built | 32 | 32 | yes |

The original `bench_inference_fixture` observed `indexes: null` because it read
`collection.inference`; segmented publication stores the dictionaries on the
segments. This replacement evidence does not use that old example field. It
proves selection, returned membership and zero-source-read execution through
the published query interface.
