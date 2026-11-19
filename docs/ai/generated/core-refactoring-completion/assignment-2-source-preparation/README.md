# Assignment 2 source-read comparison

This evidence runs the same three queries against one fixture at A1 and the
repaired Assignment 2 source. It records public result counters separately
from a narrowly filtered OS trace of successful source-file opens and reads.

| Revision | SHA |
|---|---|
| A1 | `b4172a1749750ee1c87ef74a56f5ee7cec6c40ee` |
| Repaired source | `d07b700a7188efd6b45ec542f197b78a211cb88a` |

The fixture generator sets every source file to Unix timestamp
`1795097068`. It reproduces these hashes:

| File | SHA-256 |
|---|---|
| `00.json` | `2395f83ab35cb304c13c9e0d19d5d8a92aade289636b65187a1b119ce492a1b2` |
| `01.json` | `1c827c8a52c9796f0723c8570c549e8265d6df01cd3ea556d038eccc8030dff9` |
| `02.json` | `ab2367c55636becc13a1cfbb6563100cbcbe98162c184cc01f0ee369031feeb9` |
| `03.json` | `6340d842b05dc113266cebdaef25d78aaa9f3d66d04f331bfee5fb9040409e26` |

## Reproduction

Use an explicitly named evidence directory:

```sh
evidence_dir=/tmp/objekte-assignment-2-evidence
rm -rf "$evidence_dir"
mkdir -p "$evidence_dir/a1-markers" "$evidence_dir/candidate-markers"
python3 fixture.py "$evidence_dir"
sha256sum "$evidence_dir"/data/*.json
stat -c '%Y %n' "$evidence_dir"/data/*.json
```

In the A1 checkout, copy `read_probe.rs` to
`crates/core/examples/assignment_2_read_probe.rs` and run:

```sh
strace -f -ttt -yy -e trace=openat,read,pread64,lseek,close \
  -o "$evidence_dir/a1-trace.log" \
  cargo run --quiet --locked -p objekte-search-core \
  --example assignment_2_read_probe -- \
  --root "$evidence_dir" \
  --expected-sha b4172a1749750ee1c87ef74a56f5ee7cec6c40ee \
  --trace-dir "$evidence_dir/a1-markers" \
  > "$evidence_dir/a1-reads.json"
```

Repeat in the repaired checkout with:

```sh
--expected-sha d07b700a7188efd6b45ec542f197b78a211cb88a \
--trace-dir "$evidence_dir/candidate-markers"
```

The probe derives its tested SHA with `git rev-parse HEAD` in the checkout and
fails unless it equals `--expected-sha`. It writes query start/end timestamps
for `strace` filtering. Extract compact reads for each query:

```sh
python3 trace_io.py \
  --trace "$evidence_dir/a1-trace.log" \
  --markers-dir "$evidence_dir/a1-markers" \
  --fixture-root "$evidence_dir" \
  --output "$evidence_dir/a1-trace.json"
```

Repeat for the candidate. Validate both recorded runs and the source traces,
including all negative self-tests:

```sh
python3 -O verify.py \
  --input-dir docs/ai/generated/core-refactoring-completion/assignment-2-source-preparation \
  --self-test
```

## Public counters

These fields are retained as historical observations. They do not represent
complete physical I/O for the combined queries.

| Query | Count / returned | Opens | Distinct files | Bytes | Parses | Selected payload |
|---|---:|---:|---:|---:|---:|---:|
| `exact_page` | 2 / 1 | 1 | 1 | 0 | 1 | 0 reads, 0 bytes |
| `combined_old` | 3 / 3 | 3 | 6 | 165 | 3 | 3 reads, 165 bytes |
| `combined_number` | 2 / 2 | 3 | 5 | 167 | 3 | 2 reads, 119 bytes |

Adding selected-payload bytes explains the physical totals below. The
impossible 5/6 distinct-file values are the inherited
[counter-accounting defect](../../../issues/bug/query_source_counter_semantics.md).

## Successful source I/O

The trace contains only successful `openat`, `read`, `pread64`, `lseek` and
`close` operations for the four fixture paths inside each query interval.
A1 and the repaired source produced identical ordered opens and reads.

| Query | Opens | Distinct paths | Read ranges | Bytes |
|---|---:|---:|---|---:|
| `exact_page` | 1 | 1 | `00@0+60` | 60 |
| `combined_old` | 6 | 3 | `00@0+60, 01@0+59, 03@0+46` twice | 330 |
| `combined_number` | 5 | 3 | `00@0+60, 01@0+59, 02@0+48, 00@0+60, 01@0+59` | 286 |

The exact page reads only `00.json`; it does not read the other matching
logical file. The combined queries retain repeated range reads rather than
collapsing them. `record_parses` and the isolated combined-scan control remain
separate evidence from the successful byte reads.

## Verifier controls

`verify.py` validates each revision independently, requires exactly one result
for each expected query, rejects missing/duplicate/unknown fields and values,
and writes `results_equal: true` only after validation. Its `--self-test`
mode exercises paired empty, missing, duplicate and unknown results plus a
one-sided count mutation in temporary directories under `python3 -O`.

The production JSON ownership review is in
[`json-use-inventory.md`](json-use-inventory.md).
