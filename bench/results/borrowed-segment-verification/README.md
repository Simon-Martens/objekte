# Borrowed segment verification measurements

Baseline: `5166876c7228f4fe2f8f064861cdc8a88275ad18` (the completed candidate-verification predecessor and this task's base). Final implementation: `47931f0cdfefb860b0a0cac17ae06e068daf8a1e`. The same extended `bench_candidate_verification` source was copied into a detached baseline worktree; it is committed only on the integration branch. Rust/Cargo 1.99.1, `--release --locked`, four query workers. Runs were serial on the same machine with OS cache warm-up. The verified-result cache was disabled for focused and uncached Musenalm measurements; warm Musenalm used 64 MiB. All focused rows used evidence and one untimed warm-up, seven timed repetitions, and a separate allocation-counted pass. The complete 194-query Musenalm suite used three repetitions in each profile. Timing samples, minimum, median, and maximum are in the JSONL/CSV files.

## Reproduce focused runs

The `fixtures/` manifests record corpus SHA-256 and byte counts. Four synthetic fixtures use the existing generator:

```sh
python3 bench/generate_candidate_verification_fixture.py --root /home/simon/source/objekte-bench-borrowed-segment-verification/plain --records 2048 --length 32
python3 bench/generate_candidate_verification_fixture.py --root /home/simon/source/objekte-bench-borrowed-segment-verification/escaped --records 2048 --length 32 --escaped
python3 bench/generate_candidate_verification_fixture.py --root /home/simon/source/objekte-bench-borrowed-segment-verification/duplicates --records 512 --length 32 --duplicates 256
python3 bench/generate_candidate_verification_fixture.py --root /home/simon/source/objekte-bench-borrowed-segment-verification/long-escaped --records 1024 --length 8192 --escaped
```

The fifth fixture has 512 records, each with two `text` members surrounding 128 unrelated scalar fields. Records cycle through `good/bad`, `bad/good`, and `bad/bad`; `fixtures/field-duplicates.json` records its digest. It was generated with:

```python
from pathlib import Path
root = Path('/home/simon/source/objekte-bench-borrowed-segment-verification/field-duplicates/data')
root.mkdir(parents=True, exist_ok=True)
for n in range(512):
    first, last = [('good', 'bad'), ('bad', 'good'), ('bad', 'bad')][n % 3]
    members = [f'"text":"{first}"', '"rating":5', f'"helpful_vote":{n % 10}']
    members += [f'"other_{i}":"value_{i}"' for i in range(128)]
    members.append(f'"text":"{last}"')
    (root / f'{n+1:07d}.json').write_text('{' + ','.join(members) + '}')
```

Build the example on baseline and final commits with `cargo build --release --locked -p objekte-search-core --example bench_candidate_verification --example bench_musenalm`. Run the focused query matrix for each revision/fixture, changing `--format` to `json` for `plain` and `decoded` for the others:

```sh
./target/release/examples/bench_candidate_verification \
  --root /home/simon/source/objekte-bench-borrowed-segment-verification/plain \
  --repetitions 7 --workers 4 --borrowed-focused --format json
```

The focused matrix contains required-Value identity/folded queries, direct-source identity/folded FTS, one contains FILTER, combined FTS/contains, field-scoped FTS, and high-hit logical output. `focused-comparison.md` gives all 40 matched base/final rows with candidates, hits, source bytes read, timing dispersion, allocation counts, and allocated bytes. All 40 membership digests, counts, candidate counts, source bytes/read counts, and parse counts match. Five of eight shapes are B-eligible; both value-required shapes and logical output use the combined/value route. Every candidate in a B-eligible focused case uses the direct source consumer. The `JsonDecodedScalars` value-required cases retain source-derived FTS text, because logical values can lose duplicate occurrences and number spellings.

The non-focused example runs in `base-in-memory-{plain,escaped}.jsonl` and `final-in-memory-{plain,escaped}.jsonl` measure the public owned extraction/evaluation API in memory and the repository end-to-end query. That public API remains owned by design, so its allocation counts and allocated bytes are identical at both revisions. For example, plain trusted `fts_good` took 1.846 ms baseline and 1.807 ms final, with 80,564 allocations and 5,635,482 allocated bytes in both. Escaped trusted `fts_good` took 2.366 ms and 2.333 ms, with 88,756 allocations and 5,914,010 allocated bytes in both. The optimized consumer is internal to repository execution; test-only counters separately verify zero matching-only owned-segment text bytes on B and zero collector text-copy bytes on A. Gross repository allocation counts in the focused results also include source buffers, output, evidence, and duplicate bookkeeping.

## Full Musenalm suite and build control

Run baseline and final binaries separately with these profile groups, using a fresh `--result-dir` for each revision:

```sh
./target/release/examples/bench_musenalm --root examples/musenalm --extended \
  --profiles ascii-off,unicode-off --repetitions 3 --query-workers 4 \
  --verified-cache-bytes 0 --result-dir bench/results/borrowed-segment-verification/final-suite
./target/release/examples/bench_musenalm --root examples/musenalm --extended \
  --profiles ascii-warm,unicode-warm --repetitions 3 --query-workers 4 \
  --verified-cache-bytes 67108864 --result-dir bench/results/borrowed-segment-verification/final-suite-warm
```

Both revisions passed all 194 queries in all four profiles. Across 776 case/profile rows, status, result counts, returned rows, candidates, source bytes/opens, parse counts, scanned records, and residual work counters match exactly. Median per-case final/base timing ratios: ASCII uncached 0.975, Unicode uncached 0.983, ASCII warm 1.010, Unicode warm 1.008. Build comparisons cover the shared `fold_into` change: both revisions parsed 148,486 records per alphabet and produced identical estimated index sizes (68,963,924 bytes ASCII, 71,872,324 bytes Unicode). Base/final build times in seconds were 1.578/1.243 and 1.374/1.280 in the uncached run, 1.268/1.198 and 1.285/1.289 in the warm run. No repeatable build regression was observed.

Long escaped strings retain a query-time cost. Across three serial seven-repetition comparisons, direct source combined FTS/contains median ratios were 1.135, 1.049, and 1.060; field-scoped FTS ratios were 1.101, 1.056, and 1.067. The source consumer must write decoded escapes into reusable scratch before matching; the predecessor's allocating decoder is faster on this synthetic 8 KiB escape-heavy input. The fused decoder and relevant-key-only duplicate tracking reduced the earlier slowdown without restoring per-segment allocations. High-hit logical output uses the prior combined parse, avoiding an extra full parse of most winners; its long-escaped ratios varied 1.141, 0.938, and 1.048 across the same rounds. Raw rounds are retained as `*-round{2,3}.jsonl`.

Worker-local scratch retains at most 256 KiB decoded text, 256 KiB provisional events, and 256 KiB folded text (768 KiB total per worker for these buffers); an exceptionally large record may grow temporarily and is released after that record. Tests warm capacities, then alternate long/short, escaped/plain, and folded/identity inputs; capacities and pointers stay stable during the warm portion, and copied-text counters remain zero for direct-source matching. Escaped text is decoded once into scratch, and folded text is written once into a separate reusable buffer. Evidence paths are owned only for hits when requested.
