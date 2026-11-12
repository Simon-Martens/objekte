# Index construction experiment revision: fixed protocol

Task `index-construction-experiment-revision`, fixed control B `54dcac5c3b3e1ca499180061ebcee7ad60c27101`. Current remote `master` at task start was `ab616dc6aabb9b9fee4657deb6752a6512bec9e5` and did not contain the accepted scalar folding path. B is the completed scalar-only integration result; its `bigram.rs`, `build.rs`, and `repository.rs` match measured revision `4f51c45e22ff4329c690fbbb3bad90f18b17a658`. The rejected linked arena and two-group admission implementations are absent. Every experiment branch starts at B, and earlier experimental branches remain unchanged.

The control uses the configured `/home/simon/source/objekte/examples/musenalm` corpus and locked release build of `bench_index_construction`. Record exact host/compiler/CPU affinity/governor, source and schema fingerprints, file/record/byte counts, binary hashes, code revisions and commands in each run manifest. Verify the control checkout remains clean for ordinary timing. Do not compare previous Ryzen measurements with current Intel measurements as one speedup.

Predetermined run plan:

1. Focused differential tests and kernel/real-builder measurements for vector gram generation. Reject a clear real-builder loss before full builds. For other viable candidates, complete comparable focused allocation or scheduling profiles before full builds.
2. For each viable independent candidate, one warm-up per role and five alternating fresh-process B/candidate pairs for ASCII and five for Unicode at the normal worker count. Keep profiling off. Preserve all runs. If a retention decision remains inconclusive, append four pairs to the same comparison for nine total; do not replace the original five.
3. For admission, three paired builds in each alphabet at 1, 4, 8, and 16 workers where available, omitting a duplicate normal-worker setting. Use the same small scheduling rule throughout the matrix. Run separate opt-in normal/low-worker profiles.
4. Integrate only retained candidates one at a time. Compare the final combination with B in five pairs per alphabet. If interaction changes a decision, compare with and without the suspect candidate using the same protocol.
5. Run locked workspace tests, clippy, relevant query/lifecycle checks, all extended Musenalm queries, and required-load baseline/candidate snapshot restoration. Archive raw samples and finish the task report.

Use the existing `bench/results/index-construction-optimization/2026-11-12-master-737cfb77/compare.py` driver and `crates/core/examples/bench_index_construction.rs` example. A narrow append option may be added to that driver if nine-pair extension is needed; it must preserve the first five raw pairs. All timing and heavy builds are serialized across agents. Each role has its own release target directory. Profiles and DHAT runs are excluded from ordinary timing medians. No new benchmark framework is introduced.
