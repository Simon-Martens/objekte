# Desktop delta and completion repair evidence

The exact pre-repair implementation was `f9536d8787a2788c9462cc359fa421c0ad17b9d7`. The two repair commits were `cae4d834bd` and `4cf5d759b9`; `2250398101` formats touched test code. The original checkout and master were not modified. All measurements used Rust 1.99.1 on the same host, with `--locked` builds. The detached baseline worktree was clean and normally removed after measurement.

## Desktop bytes plus evidence benchmark

`probe-before.rs` and `probe-after.rs` are temporary ignored unit probes added to `crates/desktop/src/controller/runtime.test.rs` in the respective worktrees for measurement only; they were removed after compiling and are archived here. The baseline probe repeats the exact `spawn_incremental_delta` call, materialization, and truncation sequence from `f9536d8787`. The repaired probe invokes `incremental_delta_page`, which the actual desktop worker calls. Both use the desktop's `build_query` with `ReturnMode::Bytes`, evidence enabled, no offset/count, and one freshly published JSONL segment. Each record has a 512 byte body. `loaded=0,99,100` leaves capacity for 100, one, or zero rows. The fixture build is outside the timed interval. Each sample runs in a new process, six alternating before/after pairs per scenario. Run the archived `measure_delta.py` with the two compiled test executables and an output directory to reproduce.

`runs.json` contains every sample, and `summary.json` contains medians and ranges. `materialized_hits` and `materialized_bytes` in the old probe are measured from the fully returned hit vector **before** truncation. In the repaired probe they come from `selected_payload_reads` and `selected_payload_bytes`, and are checked against the returned Bytes payloads. `peak_retained_payload_bytes` is the old vector's payload sum, a lower bound on its live payload storage, and the repaired selected payload counter's peak. It is not a general allocator or process RSS measurement. `residual_source_opens` is the query engine's source verification counter; it does not include selected result payload reads, which the separate materialization counts expose. All residual opens were zero for this blank query. Indexed or full-text queries with residual verification may open more sources than the visible page size.

| Publication | Capacity | Before median ms | After median ms | Materialized before → after | Payload bytes before → after | Residual opens |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 2,048 | 100 | 9.582 | 1.819 | 2,048 → 100 | 1,100,714 → 53,786 | 0 |
| 2,048 | 1 | 10.459 | 1.211 | 2,048 → 1 | 1,100,714 → 535 | 0 |
| 2,048 | 0 | 10.153 | 0.060 | 2,048 → 0 | 1,100,714 → 0 | 0 |
| 8,192 | 100 | 38.202 | 4.847 | 8,192 → 100 | 4,406,186 → 53,786 | 0 |
| 8,192 | 1 | 38.352 | 4.323 | 8,192 → 1 | 4,406,186 → 535 | 0 |
| 8,192 | 0 | 42.191 | 0.052 | 8,192 → 0 | 4,406,186 → 0 | 0 |

The 8,192 record query still spends more time traversing its larger posting/segment structures. The result payload work stays at the visible capacity. The zero capacity path checks appendability without executing the delta query.

## Query and construction checks

`full-query-final/` is the established 220 case × four profile Musenalm campaign with three repetitions per row, built with `cargo build --release --locked --example bench_musenalm` and run with `--root /home/simon/source/objekte/examples/musenalm --extended --profiles ascii-off,unicode-off,ascii-warm,unicode-warm --repetitions 3 --query-workers 4 --verified-cache-bytes 67108864`. `query-comparison/` compares it with the previously archived Plan 3 snapshot repair full campaign at `23d38d5e3f`. The source implementation at the actual desktop task start `f9536d8787` differs from that revision only in documents and reports. `query_correctness_summary.json` records equal resolved queries, equal result digests, and zero mismatches in all 880 statuses, match counts, page sizes, source opens/bytes/parses, exact posting matches, scans, and residual evaluations.

The one-process full campaign had timing outliers. `focused_pairs.py` ran the 15 suspect cases in both cache-off alphabets, seven repetitions per case, three alternating pairs against **exact** `f9536d8787` and the repaired code. `focused-pairs/summary.json` finds no case crossing both +10% and +0.5 ms in the paired recheck. The per-pair raw reports remain in `focused-pairs/`.

The same three paired runs also provide a comparable construction sanity check (`construction_sanity.json`). Each build parsed exactly 148,486 records before and after. Median paired build deltas were +18.3 ms ASCII and −52.2 ms Unicode, without a repeatable construction regression. The deterministic 2,050 file incremental core fixture asserts 2,050 construction reads/parses with the late inferred field. The desktop repair did not change construction code.

`validation_summary.json` records all-target workspace tests and Clippy. After the final formatting commit, `cargo test --locked --workspace --all-targets --quiet` passed again: 929 passed, 30 ignored, zero failures. `cargo clippy --locked --workspace --all-targets` exited 0 with inherited warnings. `cargo fmt --all -- --check` reports the same 1,016 inherited diff headers at the starting and final revisions after formatting edited lines; unrelated formatting was left alone.

## Native Wayland smoke

The tested `objekte-desktop` binary ran through `bash scripts/gui/run_desktop.sh prepare` in the maintained 1920×1080 headless Sway session. A task-owned, inference-enabled plain-folder fixture first had 32,769 JSON files; `f32768.json` alone contained `extra["rare.field"] = 42`, outside the first page. That opening supplied the autocomplete, builder, and predicate screenshots. The fixture was then expanded to 100,001 files and reopened to capture provisional rows and final completion. The fixture was removed after the app was closed; the screenshots are retained in `native-smoke/`.

- `provisional-rows-sequence-05.png`: 100 provisional rows with only 2,048 of 100,001 files indexed.
- `final-completion.png`: completed exact 100,001 match count and ordinary pagination.
- `zero-results-settled.png`, `completion-extra3-settled.png`: zero-result grid, then text autocomplete offering `extra["rare.field"]` as Number despite no visible-field fallback.
- `builder-field-typed-settled.png`, `builder-field-selected.png`: the same late field in builder suggestions and automatic Number type on selection.
- `builder-predicate-settled.png`, `predicate-row.png`: builder serialized `extra["rare.field"] = 42`; after clearing the unrelated zero-match full-text input, the query returned exactly `f32768.json`.

The app and task-owned VNC pointer server were stopped with `bash scripts/gui/run_desktop.sh cleanup`. No screenshot alone proves an internal source-read bound; the paired probe and focused unit tests supply that evidence.
