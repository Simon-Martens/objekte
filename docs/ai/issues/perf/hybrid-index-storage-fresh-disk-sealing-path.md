# Fresh disk build still writes and reopens unsealed segments

- **Category:** perf
- **Originating tasks:** `hybrid-index-storage` and its repair specification
- **Observed revision:** `74cab22411` on `integration/hybrid-index-storage`
- **Code:** `crates/core/src/lifecycle_open.rs` (`build_with_sources_unsealed`) and the initial managed-store write/open path

## Observation and expected behavior

The memory-only and incremental paths gained consuming sealed construction and reusable metadata proofs. A fresh synchronous disk build still chooses `build_with_sources_unsealed`: it builds mutable shards, writes them as the first managed generation, and reopens sealed readers. The [repair checkpoint](../../../../bench/results/hybrid-index-storage/repair-20261113/final-e15b/REPORT.md) identified this as the older route that can repeat equivalent metadata work. Later changes reduced several sealing and dictionary traversals, but the fresh-disk route remains intentionally distinct in current code.

The original plan asked to audit both fresh disk paths and avoid rebuilding equivalent metadata at transitions while keeping the selected residency policy and durability contract. This is a potential construction/flush/open inefficiency, not a known correctness failure.

## Reproduction and impact

Use a disposable custom store with no existing generation and run the committed `bench_managed_flush`/`bench_musenalm` examples with the same fixture, then inspect the build, flush, reopen and first-query phases. The current reports provide aggregate timing but do not isolate the redundant work in this route after all follow-ups. Its cost may matter for first use of a disk-backed index; no new speedup claim is made here.

## Why deferred and possible approaches

The user waived further performance evidence for closing the hybrid task. A future change could pass validated owned metadata directly to the first persisted reader or share the sealed-construction contract with the disk path. Any change must keep durable publication and external-byte validation intact. The separately pending `objekte_direct_snapshot_access_specification.md` covers related direct-reader architecture and should be considered before implementation. This issue is for review; it does not authorize implementation.
