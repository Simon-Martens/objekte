# Candidate verification baseline

Task: `candidate-verification-optimization`. Actual base revision:
`2bd4dbb15fd6bc7ddec178d056eefcad9b764362`. The task specification's
`crates/bench/results/` path does not exist; this directory follows the
repository's established `bench/results/` layout.

## Environment and readers

- Fedora Linux 7.2.15 on x86_64; Intel Core i9-13900K, 32 logical CPUs,
  `performance` governor.
- Rust and Cargo 1.99.1; release profile; `Cargo.lock` SHA-256
  `9c0a2ba321d688d9899ca4a0b85d511ae71aa4c78e35f13ed18e91fd7045239b`.
- Four query workers for every focused run. Verified-result cache disabled;
  each query has one warm-up before seven timed repetitions. Repository open
  warms the source files before query timing. In-memory rows exclude file
  opening, reading, and repository planning.
- The Musenalm schema manifest uses `Json` and `JsonLines` readers. Those
  suites are whole-workspace query regression and cache controls. The focused
  fixtures use `JsonDecodedScalarsReader`, the target of the combined query
  extraction work.
- The Amazon fixture copies the first 20,000 JSON files from the original
  checkout's local example. It is a representative small-file sample, not a
  full-dataset benchmark. The other fixtures are generated, deterministic,
  and identified as synthetic in their manifests.

## Reproduction

Build the benchmark binaries from the base revision:

```sh
cargo build --release --locked -p objekte-search-core \
  --example bench_musenalm --example bench_candidate_verification
```

The complete 194-case Musenalm suite ran for five repetitions in each of
ASCII/Unicode uncached and 64 MiB warm-cache profiles. Both runs had zero
case failures. Their `run-manifest.json`, `query-results.csv`,
`measurements.jsonl`, and query plans are under `base-suite/` and
`base-suite-warm/`.

```sh
./target/release/examples/bench_musenalm --root examples/musenalm \
  --extended --profiles ascii-off,unicode-off --repetitions 5 \
  --query-workers 4 --verified-cache-bytes 0 \
  --result-dir bench/results/candidate-verification-optimization/base-suite
./target/release/examples/bench_musenalm --root examples/musenalm \
  --extended --profiles ascii-warm,unicode-warm --repetitions 5 \
  --query-workers 4 --verified-cache-bytes 67108864 \
  --result-dir bench/results/candidate-verification-optimization/base-suite-warm
```

The synthetic fixtures were generated with these exact commands. Each
`fixture-*.json` records the source-byte total and SHA-256 digest.

```sh
python3 bench/generate_candidate_verification_fixture.py --root /tmp/objekte-candidate-short-plain --records 2048 --length 32
python3 bench/generate_candidate_verification_fixture.py --root /tmp/objekte-candidate-short-escaped --records 2048 --length 32 --escaped
python3 bench/generate_candidate_verification_fixture.py --root /tmp/objekte-candidate-long-plain --records 2048 --length 8192
python3 bench/generate_candidate_verification_fixture.py --root /tmp/objekte-candidate-long-escaped --records 2048 --length 8192 --escaped
python3 bench/generate_candidate_verification_fixture.py --root /tmp/objekte-candidate-duplicates-16 --records 512 --length 32 --duplicates 16 --depth 0
python3 bench/generate_candidate_verification_fixture.py --root /tmp/objekte-candidate-duplicates-256 --records 512 --length 32 --duplicates 256 --depth 0
python3 bench/generate_candidate_verification_fixture.py --root /tmp/objekte-candidate-deep-112 --records 512 --length 32 --duplicates 16 --depth 112
```

The Amazon sample was copied from the original checkout using filenames
`0000001.json` through `0020000.json`. Its local source was under
`examples/amazon-reviews-2023/data/reviews`; the fixed benchmark copy is
`/tmp/objekte-candidate-verification-amazon20k/data`. Its digest is in
`fixture-amazon20k.json`.

```sh
python3 -c 'from pathlib import Path; import shutil; source=Path("/home/simon/source/objekte/examples/amazon-reviews-2023/data/reviews"); dest=Path("/tmp/objekte-candidate-verification-amazon20k/data"); dest.mkdir(parents=True,exist_ok=True); [(shutil.copyfile(source/f"{i:07d}.json", dest/f"{i:07d}.json")) for i in range(1,20001)]'
```

For a focused run, use the corresponding fixed fixture root and a fresh
output path:

```sh
./target/release/examples/bench_candidate_verification \
  --root /tmp/objekte-candidate-long-escaped --repetitions 7 --workers 4 \
  > bench/results/candidate-verification-optimization/base-long-escaped.jsonl
```

The focused runner measures FTS, case-insensitive FTS, FILTER-only field
verification, and combined FTS/FILTER. It asserts that the in-memory and
repository paths return identical complete membership. Baseline in-memory
timings call the decoded-scalar reader's **validating** `parse_for_query` and
the existing bound evaluator over a fixed candidate sequence. A later
indexed-valid trusted extractor needs a separately labeled row. Repository
timings include index candidate selection, source reads, verification, and
result collection. Its JSONL rows
include exact membership SHA-256, counts, work counters, min/median/max, and
all seven timing samples. Gross allocation counts and allocated bytes are
captured in a separate instrumented in-memory pass, outside timed samples.

`base-short-plain-oracle.jsonl` records complete ordered results for every
focused query in locator, byte, and logical return modes, including evidence,
diagnostic references, and payloads. It has 14,757 lines and SHA-256
`29a8cf0c32f57df83f9d8ab855034d49300a35234b00f122e323913bcf6fd0ce`.
The archive was produced with `--oracle-output` on the short plain fixture.

Use `python3 bench/compare_candidate_verification.py BASE.jsonl VARIANT.jsonl`
to assert identical focused membership and source-work counters before
comparing medians and allocated bytes. The oracle archive can be compared
byte-for-byte for unchanged valid data.

## Interpretation limits

Work counters count logical record parses, not internal parser traversals,
validator calls, or source freshness probes. Targeted instrumentation and
tests must establish those removals; the counters cannot. The synthetic
long-escaped corpus deliberately stresses decoded strings and is not a
representative distribution of Amazon reviews. End-to-end timing on many
small files also includes file-open cost, so an extraction improvement can
be smaller there than in memory.

`base-discovery/` is a single-repetition exploratory run used to select
focused Musenalm cases; use `base-suite/` and `base-suite-warm/` for the
five-repetition baseline.
