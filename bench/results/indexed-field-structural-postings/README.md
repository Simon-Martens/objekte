# Plan 3 measurement archive

The implementation was measured on the Musenalm corpus at `/home/simon/source/objekte/examples/musenalm`, with the schema and admission settings in each run manifest. Plan 2 is the pushed predecessor `7c8ce508d54687afc5eeced93611b5662266b74a`; the Plan 3 production-code revision measured by the corpus runs is `491d4b4970d891619c7b5c4d14edf253858279ce`. Later commits added verification, benchmark size accounting, and documentation without changing search or construction logic. All construction campaigns used the same corpus, ASCII bigrams, 32 build workers, release compiler and fresh processes. The full query campaign used ASCII and Unicode, cache-off and warm profiles, five timed repetitions per case.

| Directory/file | Evidence |
| --- | --- |
| `construction-pairs/` | Seven alternating Plan 2/Plan 3 warm pairs, raw samples, manifests, and median/IQR summary. The benchmark used `bench_index_construction --incremental --snapshot --root /home/simon/source/objekte/examples/musenalm --alphabet ascii --workers 32 --result-dir DIR`. Snapshot files were removed after recording their byte sizes. |
| `full-220-query/` | Full 220-case, four-profile, five-repetition query campaign, including resolved queries, per-query measurements, plans, digests, and index results. Command: `bench_musenalm --root /home/simon/source/objekte/examples/musenalm --extended --incremental --profiles ascii-off,ascii-warm,unicode-off,unicode-warm --repetitions 5 --result-dir DIR`. |
| `strict-206-comparison/` | Strict protocol comparison of the unchanged first 206 cases against Plan 2. All 824 profile rows matched in status, counts, result digest and source-open ceiling. |
| `prefix-comparison.json` | The first 206 cases of the final 220-case recording compared with the Plan 2 recording after checking resolved query prefix and environment compatibility. |
| `selected-predicates/` | Focused Q207–Q220 campaign, all source-free. |
| `focused-outliers/`, `q004-isolated/` | Alternating follow-up measurements of apparent predecessor-query latency outliers. |
| `direct-profile-before/`, `direct-profile-after/` | Single direct-build attribution runs with `--profile`; these are diagnostics, not the acceptance baseline. Plan 3's direct estimate includes the new structural posting bytes. |
| `foreground-construction/` | Nine fresh processes alternating idle, Q108/Q116/Q120 foreground, and held-early-view modes using `bench_initial_concurrency`. |
| `objekte-plan3-reference-26.log` | Independent source evaluator for Q195–Q220; optimized count and page hashes matched the oracle. |
| `native-desktop-fixture.png` | Native Wayland desktop launched from this integration worktree and opened the five-file fixture. The application was stopped with task-owned cleanup. |
| `validation/` | Final workspace test, Clippy, inherited rustfmt/spacing diagnostics, and native desktop launch/cleanup logs. |

The paired acceptance results are in [the generated report](../../../docs/ai/generated/indexed_field_structural_postings_results.md). The direct-build profile is inherently noisy at one pair: its sampled posting callback covers 2,321 records, and its wall time should not replace the seven paired incremental trials. The foreground campaign is a separately instrumented construction mode, not a direct latency comparison with the no-query paired campaign.
