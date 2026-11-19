# Assignment 3 relation and integration evidence

This directory retains the evidence for `core-refactoring-completion`
Assignment 3. The tested source is
`acac94f486ee0f9d35eb5ba834d26cdc01eb1c5e`.

## Artifact index

| Path | Contents |
|---|---|
| `inventory/` | Default and all-feature B/C test lists, raw `--list` output, ignored-only lists, and the reconciled namespace mapping |
| `test-logs/` | Focused relation tests, workspace and all-feature tests, documentation, Clippy, formatting, spacing and the ignored release control |
| `relation-a2-to-c-extended/` | A2/C extended-schema samples and plans for Q102 and Q122 |
| `relation-a2-to-c-extended-hops/` | A2/C extended-schema samples and plan for typed-ID Q088 |
| `relation-probe-summary.json` | Asserted A2/C equality for ordered results, work counters and normalized plans |
| `musenalm/` | Common-corpus B/C query and construction evidence |
| `musenalm/comparison/` | Repository comparator outputs and the paired time/RSS analysis |

## Reproduction

The corpus root recorded in every run is the absolute canonical path
`/home/simon/source/objekte/examples/musenalm`. B and C used separate target
directories and were built with these commands from their respective clean
worktrees:

```sh
OBJEKTE_BUILD_REVISION=<full-revision> CARGO_TARGET_DIR=<absolute-target> \
  cargo build --locked --release -p objekte-search-core \
  --example bench_musenalm --example bench_build_musenalm
```

Query runs used:

```sh
<target>/release/examples/bench_musenalm \
  --root /home/simon/source/objekte/examples/musenalm \
  --profiles ascii-off --repetitions 3 --query-workers 4 \
  --verified-cache-bytes 0 --result-dir <side>/queries
```

Three ASCII and three Unicode construction runs per side used
`bench_build_musenalm --root /home/simon/source/objekte/examples/musenalm
--build-workers auto`. The resolved worker count was 32 on a 32-CPU host.

The extended A2/C relation probes add `--extended` and
`--cases Q102,Q122` or `--cases Q088`.

## Summary

The full comparator reports 150/150 ordered result digests and 450/450 paired
samples with no status, error, counter, query-document, workload-byte, schema
or corpus compatibility difference. Both alphabets have equal index
signatures, 148,486 records and build parses, and zero failed parses.

The broader repository schema verifier does not accept the extended benchmark
adapter field in either A2 or C schema artifact. The archived extended
artifacts are byte-identical, and `summarize_relation_probe.py` directly
asserts equality of their normalized plans, ordered digests and every sampled
counter. This limitation does not affect the required standard full-corpus
comparison, whose compatibility failure list is empty.

Run the local analysis scripts with:

```sh
python3 reconcile_inventory.py inventory inventory/reconciliation.json
python3 analyze_musenalm.py musenalm musenalm/comparison/performance-analysis.json
python3 summarize_relation_probe.py . relation-probe-summary.json
python3 capture_corpus.py /home/simon/source/objekte/examples/musenalm corpus.json
```
