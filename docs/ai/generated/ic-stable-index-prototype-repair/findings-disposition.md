# Findings disposition — R01 to R20

Task: `ic-stable-index-prototype-repair`  
Reviewed prototype: `530f4037f01bcd38f5027042e54676b543fdd230`  
Repair branch: `integration/ic-stable-index-prototype-repair`

| ID | Disposition | Code / evidence |
| --- | --- | --- |
| R01 | **Fixed** | `stable_index/build.rs` assigns canonical IDs, computes role-aware work estimates, sorts execution largest-first and runs `into_par_iter().with_max_len(1)`, then restores result order by component ID. Final trace: makespan 1.421 s, 5.36 average concurrent jobs, large catalogs on distinct workers from the first wave. Scheduling-only: original median 5.005 s vs 3.858 s candidate (22.9% better). |
| R02 | **Partial — residual** | `build.rs::BatchContribution::into_parts` moves files, records, fingerprints, typed/custom fragments, packed bigrams, domains, sort keys and relations. `source_build.rs::admit_parts` consumes these parts and does not clone the batch. However the direct builder still finalizes `CollectionState` indexes and passes a borrowed collection graph into `from_collections`; native index ownership is not consumed by conversion jobs. This is the main residual memory defect. |
| R03 | **Partial** | `admit_parts` separates metadata admission from index routing and merges posting-field owners in parallel through the shared pool. Custom posting, packed bigram, sort-key and relation owners still merge in the coordinator. No global admission clone or transaction layer was introduced. |
| R04 | **Fixed** | `sorting.rs::SortBuilder::finish_keys_only` lets the direct path consume collation keys without constructing native ascending/descending vectors. Stable sort components are built from keys and native RecordId tie-breaking. Composite sorts are separate components. |
| R05 | **Fixed** | `stable_index/memory.rs` now stores a `ComponentTree` enum of typed `BTreeMap`s. `codec.rs` defines fixed-width `FileKey`, `RecordKey`, `StateKey`, `GramKey` and fixed occurrence keys; only path/identity/scalar/number/sort/relation-target keys are unbounded. Key codec revision is 3 and descriptor key bounds are role-specific. |
| R06 | **Fixed** | Live domains are cached as `Arc<RoaringBitmap>`; `live_domain_shared` returns the shared handle, `live_domain_owned` is the explicit copy API. Typed/gram/sort/relation queries intersect shared domains in place. |
| R07 | **Fixed** | Next file ID is cached after first lookup/build. Append and delete group domain changes once per domain and identity changes once per logical ID. No per-record domain serialization remains. |
| R08 | **Fixed** | `PreparedFileChange`, `PreparedCollectionChange`, `PreparedPostingAdd` and `replace_prepared_file` install typed, length, state, gram, sort/composite, relation occurrence/target/state and coverage facts. Unit tests cover unchanged and changed replacements, native rebuild comparison, and reopened state. |
| R09 | **Fixed** | `delete_file_by_path` uses the full path entry, counts retirements per collection, removes zero-record paths, retires physical files independent of record count, and returns qualified `RecordRef`s. |
| R10 | **Fixed** | Identity policy is persisted as a typed enum. Move validates only actual collection memberships, handles source==destination, dead/absent source, zero-record destinations and non-UTF-8 paths; RelativePath/filename-sensitive moves require replacement. `path_entry` distinguishes absent from zero-record entries. |
| R11 | **Fixed** | Semantic APIs return `Restriction::NoRestriction` or `Complete(bitmap)`. Values/lengths/states check their own coverage flags; complete-empty is distinguishable from unavailable. Text candidate lookup checks text coverage. Regression test covers unindexed/incomplete/complete-empty. |
| R12 | **Fixed** | The prototype stores the first invalidation reason, rejects tree/query/mutation/save/load use when unusable, and `replace_prepared_file` marks unusable on post-application failure. Fault test proves queries and saves are blocked after the first change. |
| R13 | **Fixed** | Standalone save/save_as use `codec::publish_component_file`: sibling create-new temp, write/verify, sync, rename, parent sync. Unloaded copies use an atomic temporary copy. Fault tests prove no final partial file and a retry succeeds. |
| R14 | **Fixed** | `write_component_file` streams payload bytes through one SHA-256 pass and writes the final envelope afterwards. No pre-write full payload hash remains. |
| R15 | **Fixed** | `write_snapshot_to` returns staged `ComponentFileInfo` values and does not mutate saved paths/dirty flags. Metadata is committed only after rename. Restoration failure installs `Failed`; empty placeholder trees are never exposed. Fault test verifies dirty/path state is unchanged after failure. |
| R16 | **Fixed (within prototype)** | Descriptors include text alphabet/extractor/normalization/segment identity, composite constituents, target/ordering identity, key bound, coverage, revision, dependencies and detached provenance. Configuration hashing is explicit structured hashing over sorted fields/bindings/relations/sorts/overrides/composites, not Debug strings. Coverage/revision is bumped on mutations. |
| R17 | **Fixed** | Component ID decoding is checked u64→u32; manifest validation checks unique IDs/paths/logical identities, collections, revisions, live≤high-water, slot namespace, dependency existence, same collection and same namespace. |
| R18 | **Fixed for measured fields; residual null-policy** | BuildTree observes insert and bitmap-encode calls at their call sites; build records include entry count, memory length, Vec capacity, bitmap encodes, metadata encodes and insert count. Stage timings, source reads/parses, trace and memory counters are recorded. A few unavailable low-level diagnostics are still emitted as zero rather than null. |
| R19 | **Partial** | Primitive mode preloads component and live domain separately, runs 200 repetitions, consumes results, propagates errors and reports median/p95/min/max. Matched native primitive distributions were not run. |
| R20 | **Fixed** | Prepared replacement tests compare typed/state/sort facts after native rebuild and reopen; composite sorts are implemented with native constituent comparison/directions/ties and tested. Lifecycle reports dead slots and payload growth. Relation prepared facts are supported by the generic prepared update representation and relation components. |

## Residual actions before a full acceptance claim

1. Implement an owned `CollectionState`/component conversion job graph so native
   indexes are released as each stable component is built (R02).
2. Move custom-binding, bigram, sort-key and relation owner merges out of the
   coordinator (R03).
3. Add matched native primitive distributions (R19).
4. Emit explicit null plus reason for every unavailable diagnostic counter (R18).
