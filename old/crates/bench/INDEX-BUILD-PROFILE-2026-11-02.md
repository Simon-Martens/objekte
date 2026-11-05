# Index construction stage profile — 2 November 2026

This profile measures cold, cache-free builds of five example repositories. Each configuration ran three times in a separate release process. The [raw results](results/index-build-profile-2026-11-02.ndjson) contain every stage measurement, compressed field size, snapshot size, and run. Medians below are computed per column; they need not add to a median total.

## Method

Run `python3 crates/bench/run_index_build_profiles.py --runs 3` from the repository root. The runner builds `index-build-profile` with `objekte-core/index-timings`, alternates profile order, and runs one build per process. `FilePicker::profile_collection` ignores saved caches. It builds the normal index and any declared FTS fields from the source files, then writes a temporary base and secondary snapshot under `target/`. The separately reported secondary serialization is a second sidecar-only save after that full snapshot save. The temporary snapshot is removed when the process exits.

The machine was a 13th Gen Intel Core i9-13900K with 32 logical CPUs, Linux 7.2.13, and Rust 1.99.1. The scan pool used 32 workers. Input files were in the filesystem cache for much of the run; this is a warm-storage, cold-index profile. Peak RSS is Linux `VmHWM` sampled immediately after construction, before snapshot saving. Compressed index bytes count bigram filter heap storage, excluding file inventory, typed indexes, ordered views, and allocator overhead.

The generated examples were materialized with `prepare_amazon_reviews.py` and `prepare_nyc311.py` at their default one-million-record limits. Amazon used the Musical Instruments archive with SHA-256 `0e7bf61672d2a984b153d6999ac7321fb4b29642f05c04496f54507437bbbfee`. NYC 311 was fetched from its live API through key `46420065`; its data can change on a fresh import. Their exact import-manifest hashes are included in every raw result. Leipzig has 400,000 pinned multilingual records. Musenalm and Lenz use the checked-in examples at commit `a95ff29d94`.

The `base` profiles use each repository's existing schema or schema-free discovery. The FTS profiles add the following string fields for this measurement without rewriting source schemas:

| Profile | FTS fields |
|---|---|
| Lenz | `Eintrag`, `Autor` (plain) |
| Musenalm | `annotation` (Markdown), `preferred_title` (plain), `title_statement` (HTML), `contributors[].annotation` (XML) |
| Leipzig | `text` (plain, UTF-8 byte bigrams) |
| Amazon | `title`, `text` (plain) |
| NYC 311 | `complaint_type`, `descriptor`, `resolution_description` (plain) |

Leipzig used `utf8_bytes` in both profiles. The other repositories used ASCII bigrams.

## Build and storage medians

| Profile | Files | Build wall ms | Scan wall ms | Peak RSS MiB | Compressed MB | Secondary save ms | Secondary file MB |
|---|---:|---:|---:|---:|---:|---:|---:|
| Lenz base | 2,133 | 22 | 13 | 16 | 0.3 | 1 | 0.0 |
| Lenz FTS ×2 | 2,133 | 35 | 26 | 21 | 1.0 | 7 | 0.9 |
| Musenalm base | 133,948 | 1,157 | 261 | 390 | 15.8 | 92 | 32.0 |
| Musenalm FTS ×4 | 133,948 | 1,505 | 486 | 780 | 52.7 | 154 | 71.9 |
| Leipzig base | 400,000 | 1,726 | 378 | 1,234 | 36.5 | 11 | 1.9 |
| Leipzig FTS ×1 | 400,000 | 3,987 | 977 | 2,551 | 109.6 | 208 | 111.3 |
| Amazon base | 1,000,000 | 2,537 | 968 | 1,269 | 147.9 | 16 | 4.9 |
| Amazon FTS ×2 | 1,000,000 | 4,790 | 2,646 | 3,949 | 430.5 | 501 | 376.6 |
| NYC base | 1,000,000 | 2,467 | 886 | 771 | 110.9 | 16 | 4.9 |
| NYC FTS ×3 | 1,000,000 | 4,237 | 2,593 | 1,951 | 379.6 | 400 | 298.6 |

The full snapshot save, which includes the base and sidecar, is separate from the build: its median was 392 ms for Musenalm FTS, 794 ms for Leipzig FTS, 2,136 ms for Amazon FTS, and 2,095 ms for NYC FTS. The raw results retain all three samples.

## Wall stages and the missing compression time

These are actual wall intervals, in milliseconds. `Main` is the existing `bigram_compress_nanos` metric; `rendered` is the combined rendered filter; `field` includes both normal and rendered filters for every declared field. Inventory, typed, and view stages are shown because they can dominate the build even when compression is fast.

| Profile | Inventory | Main compression | Rendered compression | Field compression | Typed | View order |
|---|---:|---:|---:|---:|---:|---:|
| Musenalm base | 155 | 14.7 | 0 | 0 | 187 | 510 |
| Musenalm FTS ×4 | 154 | 12.7 | 14.4 | 80.4 | 192 | 515 |
| Leipzig base | 465 | 823.2 | 0 | 0 | 0 | 0 |
| Leipzig FTS ×1 | 467 | 0.3 | 820.8 | 1,668.0 | 0 | 0 |
| Amazon base | 1,188 | 170.5 | 0 | 0 | 0 | 0 |
| Amazon FTS ×2 | 1,175 | 65.7 | 171.2 | 539.7 | 0 | 0 |
| NYC base | 1,338 | 61.9 | 0 | 0 | 0 | 0 |
| NYC FTS ×3 | 1,166 | 59.4 | 62.0 | 168.0 | 0 | 0 |

The old metric reports only main compression. For Musenalm FTS, total compression is about **106 ms**, versus 13 ms reported by that metric. For Leipzig FTS, it reports **0.3 ms** while the rendered and field filters require another **2,489 ms**. Leipzig's text is removed from the general normal filter, so its main compression becomes almost empty. The new feature-gated metrics expose rendered and field compression separately.

## Summed worker CPU time

The following values are seconds of **summed thread CPU time** across the 32 scan workers, not build wall time. Read includes secure file opening and before/after metadata checks; parse is Sonic JSON parsing; values is the general decoded-value projection; text includes per-field extraction and rendering; insertion is split among main, combined rendered, and field postings.

| FTS profile | Read | Parse | Values | Field text | Main insert | Rendered insert | Field insert |
|---|---:|---:|---:|---:|---:|---:|---:|
| Lenz ×2 | 0.03 | 0.01 | 0.02 | 0.13 | 0.03 | 0.07 | 0.11 |
| Musenalm ×4 | 0.91 | 0.29 | 2.44 | 4.25 | 0.76 | 1.01 | 1.46 |
| Leipzig ×1 | 2.73 | 0.28 | 0.25 | 4.57 | 0.12 | 3.39 | 6.48 |
| Amazon ×2 | 12.80 | 1.02 | 2.74 | 31.44 | 3.74 | 8.61 | 14.25 |
| NYC ×3 | 5.94 | 1.68 | 7.23 | 34.64 | 5.74 | 7.97 | 9.14 |

The raw results also contain summed per-worker elapsed time. For example, Leipzig FTS field insertion spent 13.79 seconds of summed elapsed time but 6.48 seconds of summed CPU time, suggesting time lost to scheduling or memory stalls; that interpretation needs a hardware-counter profile before treating it as proven memory-bandwidth contention. The read elapsed/CPU pairs were 0.94/0.91 seconds on Musenalm, 2.72/2.73 on Leipzig, 14.73/12.80 on Amazon, and 6.10/5.94 on NYC. Most read time in these runs was charged to the workers rather than spent waiting for storage.

## Next optimization targets

- **Leipzig:** compression is the clearest wall bottleneck. The two field filters and combined rendered filter take about 2.49 of 3.99 seconds, while the main filter appears almost free.
- **Musenalm:** field extraction/rendering uses 4.25 worker CPU seconds, but view ordering (515 ms) is the largest single serial wall stage. All compression together is about 106 ms.
- **Amazon and NYC:** field extraction/rendering uses 31–35 summed worker CPU seconds, and field insertion adds 9–14 seconds. Peak build RSS reaches 3.95 GiB on Amazon and 1.95 GiB on NYC. Compression and secondary serialization also deserve attention but are smaller than the parallel projection work.

The stage timers are compiled out of ordinary builds. The profile adds timing calls, so its absolute build time is not a zero-overhead production latency measurement. The parallel stages interleave per document; their recorded elapsed times are sums across workers rather than independently isolatable wall intervals. Only whole build, scan, inventory, compression, typed/view ordering, and snapshot saving have direct wall intervals. CPU time comes from the Unix thread CPU clock; the raw format records whether it is available. No source files or collection schemas were modified by the benchmark.
