# Assignment 2 source-read comparison

This probe compares A1 and the Assignment 2 source at C with identical fixture
bytes, worker count, verified-cache size, query text and payload mode.

| Revision | SHA |
|---|---|
| A1 | `b4172a1749750ee1c87ef74a56f5ee7cec6c40ee` |
| Candidate C | `c34294489f0011ca7a0009b54e0af5a6d6b600b3` |

The four source files have modification-time seconds `1795097068` and these
SHA-256 hashes:

| File | SHA-256 |
|---|---|
| `00.json` | `2395f83ab35cb304c13c9e0d19d5d8a92aade289636b65187a1b119ce492a1b2` |
| `01.json` | `1c827c8a52c9796f0723c8570c549e8265d6df01cd3ea556d038eccc8030dff9` |
| `02.json` | `ab2367c55636becc13a1cfbb6563100cbcbe98162c184cc01f0ee369031feeb9` |
| `03.json` | `6340d842b05dc113266cebdaef25d78aaa9f3d66d04f331bfee5fb9040409e26` |

## Reproduction

```sh
fixture_root=$(mktemp -d)
python3 fixture.py "$fixture_root"
```

Copy `read_probe.rs` to `crates/core/examples/assignment_2_read_probe.rs` in
the A1 worktree and run:

```sh
cargo run --quiet --locked -p objekte-search-core \
  --example assignment_2_read_probe -- \
  --root "$fixture_root" \
  --tested-sha b4172a1749750ee1c87ef74a56f5ee7cec6c40ee \
  > a1-reads.json
```

Copy the same file to the candidate worktree and run:

```sh
cargo run --quiet --locked -p objekte-search-core \
  --example assignment_2_read_probe -- \
  --root "$fixture_root" \
  --tested-sha c34294489f0011ca7a0009b54e0af5a6d6b600b3 \
  > candidate-reads.json
```

Validate the six recorded query executions:

```sh
python3 verify.py
```

| Query | Count | IDs | Files opened | Distinct files | Bytes read | Record parses | Residual |
|---|---:|---|---:|---:|---:|---:|---:|
| Exact indexed page | 2 | `00.json` | 1 | 1 | 0 | 1 | 0 |
| `old` + combined filter | 3 | `00`, `01`, `03` | 3 | 6 | 165 | 3 | 3 |
| `1e0` + combined filter | 2 | `00`, `01` | 3 | 5 | 167 | 3 | 3 |

The results are byte-for-byte equal across A1 and C after excluding the
revision SHA. `verify.py` also asserts the expected values independently.

The production JSON ownership review is in
[`json-use-inventory.md`](json-use-inventory.md).
