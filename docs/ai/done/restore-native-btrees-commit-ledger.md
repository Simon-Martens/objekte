# Complete B..H disposition ledger

Companion to [the main plan](restore-native-btrees-memory-only.md) and [the execution handoff](restore-native-btrees-handoff.md).

Range: `da8fde68472b8646a63a05b3b3654c58de877301..a0a0691897c6ff24985fc4db9ae51f5d77a89899`.
Order: `git log --reverse --topo-order`. Exactly 236 rows: 197 non-merges and 39 merges.

This is a final-state disposition, **not a cherry-pick or revert script**. Start at the execution head and edit forward. A historical implementation removed by a later row must stay removed.

Codes:

- **K**: keep surviving behavior; K-numbers refer to the main plan.
- **A**: mixed change; apply only the stated native/live behavior.
- **D**: remove the implementation with persistence/encoded readers; keep any historical evidence in the commit.
- **H**: retain historical documentation/results/issue history. Update active docs separately.
- **M**: integration merge; both parents are accounted for in this ledger. No additional remerge resolution was reported.
- **MR**: merge with an additional resolution; explicit disposition below.

The merge audit covered the whole range, without a path filter. `git log --merges --remerge-diff` exposed exactly two nonempty resolutions, both listed as MR. A merge row does not mean reverting the merge.

| Commit | Code | Final-state instruction |
| --- | --- | --- |
| `bfca5ba9d205` | A | K05: retain borrowed logical bitmap, inference-copy API and timing; discard storage contracts as active architecture. |
| `083fc1e6b0df` | D | Delete immutable byte-segment/ordered-block primitives. |
| `e6475f6c5fa9` | D | Delete generation manifest storage and locking. |
| `1eea29c80731` | D | Delete deprecated-snapshot recognition and cleanup. |
| `4c2632bc1459` | D | Delete full-residency serialized opening. |
| `54bcf9bf85d5` | D | Delete sealed posting dictionary adapter. |
| `a0c9bddbd6cf` | D | Restore native gram maps; delete packed gram backend. |
| `412042650adc` | D | Delete stored posting-validation evidence. |
| `73380f3ce145` | D | Delete storage/residency CLI parsing. |
| `33790d0ecf99` | A | Fresh source construction is unconditional; remove cache-aware repository opening. |
| `8abf481fe224` | M | CLI/core integration. |
| `69f039f94a90` | A | K25/K26: retain fresh opening without probes; remove storage policy plumbing. |
| `05bdbdc35d87` | K | K04: legacy TUI snapshot-save state stays deleted. |
| `077150a16cf2` | D | Delete TUI storage selector/reopen operation; keep ordinary source opening. |
| `11e7f02e7fab` | A | Migrate semantic process tests to fresh memory; delete storage policy cases. |
| `d51cc267ef10` | A | Rewrite active CLI/TUI docs to memory-only operation. |
| `03dc3e23bcf3` | A | Preserve source/schema selection; writer-protection API disappears with all index writers. |
| `de4dfe746ced` | D | Delete checked gram reader installation. |
| `885fab1d2a13` | A | Restore native gram access; retain real source/query errors, not decoder error plumbing. |
| `a64bda80c76f` | D | Delete persisted masks/generation conflicts; keep live active masks already native. |
| `0be3dffa1413` | A | K26: retain ordinary exclusions in definitions; delete resolved storage identity pairing. |
| `5a0d8c57b6eb` | D | Delete managed-store filesystem admission. |
| `b927570ff72c` | M | Live-mask/storage integration. |
| `5cda86d630f6` | A | Keep source path context in discovery errors; delete store-path preparation. |
| `e1fa9a2daa49` | D | Delete managed-restoration repository owner construction. |
| `cc61b12f7e69` | D | Delete generation catalog publication. |
| `e9967ff19b35` | H | Preserve historical contract; remove active persistence guidance. |
| `cf0dd92e3067` | D | Delete persisted collection/ordinal metadata; retain live ordinals. |
| `ae989cc9631a` | D | Delete encoded record/sort/relation sections. |
| `9f8a8dfe6bb4` | M | Codec metadata integration. |
| `404b7932f0f4` | D | Delete generation catalog sidecar codec. |
| `68a981e300ac` | M | Catalog codec integration. |
| `3ef9f6eed1bc` | D | Delete owned serialized segment codec. |
| `412a4add26ca` | M | Segment codec integration. |
| `7cf99968fa8b` | D | Delete persisted historical metadata references. |
| `6b71fc745566` | M | Historical-reference integration. |
| `947d66e10b87` | D | Delete managed opening/synchronization; retain native lifecycle operations. |
| `058de13dedeb` | M | CLI/core integration. |
| `56b178cdad1b` | A | Delete managed streaming; retain inference-report assertions in native partition tests (K28). |
| `e4450ba89f1f` | D | Restore completed native posting maps; delete install-sealed/materialize dual state. |
| `b0c0003a3f95` | A | Replace byte-dictionary query routes with native lookups; retain semantic query checks. |
| `89cde537bf13` | A | Restore native bounded B-tree ranges and value-page behavior; delete sealed seeks. |
| `3b04bcd041c9` | M | Sealed-query integration. |
| `4f3c8edd5aed` | M | CLI/core integration. |
| `b5b0d594618e` | D | Deleted codec initialization. |
| `7d06b0e3475c` | M | CLI/core integration. |
| `2a24af9f96b3` | D | Delete CLI/TUI managed storage wiring. |
| `23787ba7b171` | A | K25: retain progressive initial opening through native InitialBuildTask; delete managed wrapper. |
| `6f29454d256a` | M | Progressive CLI integration. |
| `497bfbbf1176` | A | K25: retain progressive TUI status/cancellation; remove managed mode labels. |
| `4962cb3785f8` | A | Fresh native construction becomes sole opening path. |
| `66f7188bf224` | M | CLI/core integration. |
| `25a751860e1b` | A | Preserve CLI semantic assertions; delete full-residency reopen/status cases. |
| `45c7505215a1` | D | Delete sealed segment opening. |
| `0af9b78c2bdb` | A | Preserve bounded posting-value pages, cursor ownership and order using native ranges. |
| `0861ca973d67` | MR | Resolution chose empty builder maps for sealed decoding in hybrid_segment_codec.rs. Delete entire backend; no live resolution to replay. |
| `679c83b720f0` | D | Delete managed sealed-query selection. |
| `bb7ec8a75f87` | M | CLI/core integration. |
| `3edc9fb91d35` | A | Retain loader/schema assertions; remove typed storage policy tests. |
| `527165108ae8` | K | K04: obsolete CLI snapshot entry points remain deleted. |
| `d339288219aa` | A | Preserve source Open preparation and ordinary preferences; delete storage fields/controls. |
| `86f1f381c4ef` | M | Desktop/core integration. |
| `d1ed3969b727` | D | Delete desktop storage-choice routing. |
| `407e209a6164` | A | Delete managed status/flush-on-close; preserve worker shutdown and close gating (handoff D3/D6). |
| `092ec002c774` | M | Desktop/core integration. |
| `07a4b156a8da` | A | K25: retain progressive desktop open and cancellation through native builder. |
| `4ee4e1162710` | K | K04: legacy desktop snapshot open routes stay deleted. |
| `6c8687f0c102` | M | Desktop integration. |
| `f725dcc8f209` | M | CLI/core integration. |
| `352600791a58` | M | CLI integration. |
| `07caad5caf8c` | D | Delete source-free encoded dictionary reencoding. Native source-free maintenance stays. |
| `534b9a8a43e1` | D | Delete owned immutable byte sealing. |
| `21c3242613f0` | M | Sealed-query integration. |
| `ea6ec585cfd0` | K | K28: preserve metadata-FTS no-source-read assertions on native views. |
| `8ae51cb64cd2` | M | Sealed-query test integration. |
| `5342a54a08c6` | A | K04/K28: retain legacy deletion and oracle coverage; remove managed test wrappers. |
| `3b81e322810e` | M | Desktop/core integration. |
| `d6fe6083e5ff` | K | K04: legacy snapshot UI actions/workers stay deleted. |
| `196660e8b090` | K | K04: legacy desktop snapshot backend stays deleted. |
| `fa562a764fb7` | A | Delete replacement-open flush; preserve previous session until new open succeeds and stale-worker cleanup. |
| `796667044c95` | M | Desktop integration. |
| `8b166cc7463c` | D | Delete all memory-only serialization/sealing switches and paths. |
| `69bec5717d20` | A | Rewrite current desktop docs; retain ordinary native GUI instructions. |
| `4d5d26269dd9` | M | Desktop/core integration. |
| `53d9e24184ab` | A | Retain fresh-source defaults; remove storage-policy assertions. |
| `5d083edd1f13` | M | Desktop tests/docs integration. |
| `d0938dfbe522` | D | Delete fresh-build committed-reader installation. |
| `f1d0e5096d04` | A | Retain query campaign correctness/provenance; remove storage-mode matrix. |
| `70a055c0026b` | D | Delete owned-seal validation shortcut. |
| `044ad6c17ff9` | A | Delete synchronization status; retain nonblocking live status via IndexStatus. |
| `33d1af0a8e35` | D | Delete decoded-posting cache, budget and counters. |
| `5f4493951343` | D | Delete catalog-history rebase on flush. |
| `389375b06d06` | D | Delete runnable managed flush benchmark. |
| `a7cdb101cb93` | H | Preserve gate measurements and recorded regressions. |
| `a135638933f6` | A | K06: retain provenance, preparation/install timing; remove sealing timing. |
| `55e1fbfd22e3` | A | Retain matched incremental query comparison methodology; remove runnable storage backend branches. |
| `9848115b1b96` | A | K06: keep accurate binary revision/provenance, native mode labels only. |
| `9f51e668fe94` | D | Delete consumption into sealed readers; native segment ownership moves directly. |
| `7965fcb7529a` | D | Delete encoding validation reuse. |
| `19f560ffc4e1` | D | Delete sealed-section persistence reuse. |
| `cd58d0431092` | D | Delete constructed reader posting proofs. |
| `63fb0c0db304` | K | K12: retain native broad-ID union and sparse lookup strategy. |
| `78a33da69379` | D | Delete decoded cache recency eviction. |
| `e27dbc07d3d7` | D | Delete decoded cache occupancy APIs and benchmark fields. |
| `025bcefe04ee` | D | Delete constructed posting payload encoding. |
| `b89f72953b95` | D | Delete encoded leaf key streaming. |
| `56ac0b44ceb6` | D | Delete typed managed-store errors; retain ordinary lifecycle errors. |
| `467f24bcd05f` | A | Preserve gram correctness during compaction/coalescing via native maps and migrated tests. |
| `14635a01a735` | K | K18: retain unaffected-shard Arc reuse. |
| `ed00d4987cd4` | K | K19: retain one PartitionRouting map and slot validation. |
| `e56f6654e9fb` | D | Delete encoded-key decode shortcut. |
| `d0ed1313994e` | D | Delete owned encoded buffer transfer. |
| `ff6c73e9478d` | A | K20: scatter native grams once across partitions. |
| `e8f2b1ea487e` | A | K23: count/retirement/size maintenance, hysteresis, current zero-record retention; native byte estimate. |
| `a52213f29ddf` | H | Preserve paired construction/query diagnostics archive. |
| `c0a2147bea50` | D | Delete sealing-only phase timers. |
| `39c705de99f9` | D | Native gram tables remain completed indexes; delete release-on-seal. |
| `c602143e4f41` | D | Delete borrowed encoded-ID path; retain K12 native union. |
| `8331da80d1e1` | K | K23: retain retirement maintenance after deletion/lost membership. |
| `54b0d0ae2826` | D | Delete encoded-section reservation. |
| `713ca48f14b0` | D | Delete encoded-section owned-buffer retention. |
| `678b61ed4e48` | D | Delete encoded gram comparisons. |
| `027b3e2fef86` | D | Delete sealed gram presence filter; native HashMap absence is exact. |
| `0f36f5d03010` | D | Delete deferred metadata encoding. |
| `e15b3f5c4b0e` | K | K21: retain one-pass relation scattering including positions/domains. |
| `a9ab5ec2841c` | D | Delete typed flush failures. |
| `e397f8f30469` | H | Preserve repair measurements. |
| `09154558b74e` | D | Delete encoded dictionary key arena. |
| `003297081abe` | D | Delete direct serialized payload append. |
| `7279796a351e` | D | Delete encoded scalar comparator; use native ScalarKey ordering. |
| `fa8394f6a7ca` | A | Borrow native bitmaps for unions/intersections; remove decoded cache wrapper. |
| `376dd75b611a` | D | Delete seal-time binding/fallback proofs; keep normal construction validation. |
| `980a8831dc98` | K | K22: retain one-pass sort/domain scattering. |
| `5ca0598b0dcf` | K | K22: retain inferred/custom postings and inference projection. |
| `23c4b1a5886a` | D | Delete static-build sealing; publish native shards. |
| `2ba53f3a2035` | D | Delete encoded gram-presence construction. |
| `9835f0c4e7b9` | K | K11: retain inline single source candidate ranges. |
| `2188fbd178cf` | A | Keep memory mutation/maintenance benchmark; delete managed load/flush modes. |
| `82cc05c21c11` | K | K18: retain unaffected collection-group reuse. |
| `3bd8b74d795a` | A | K24: retain bounded batches/reports; native accounting per D5. |
| `ceba166b727f` | D | Delete external gram-presence validation. |
| `471b79c35bb6` | D | Delete unchanged-flush benchmark mode. |
| `155bcf8659b8` | H | Preserve allocation/query performance archive. |
| `ca183a1ed84d` | H | Preserve performance follow-up completion record. |
| `c977710f2d2e` | D | Delete decoded-posting domain proofs; retain native domain invariants. |
| `8ca2851ab8d9` | A | Delete generations/stamps/proofs; preserve lifecycle cancellation/quiescence and nonstorage UI behavior per D3/D6. |
| `32b00f5137cd` | H | Preserve query optimization control measurements. |
| `5020aa328d15` | A | K08: declared structural facts and values_indexed semantics; delete persisted binding flags. |
| `0a80e830a4ea` | K | K09: retain guarded declared array-state routing and tests. |
| `a09368464796` | K | K10: raw-presence self-comparison, guards, differential tests. |
| `3b959f10555a` | K | K11: lazy evidence and file-run grouping with out-of-order fallback. |
| `4a1c2a8883cc` | D | Delete decoded-posting cache atomic clock. |
| `aaf732dd9329` | K | K13: segmented EXPLAIN and distinct relation list/member counters. |
| `293a10498c63` | H | Preserve query campaign and raw benchmarks. |
| `74cab22411dd` | H | Preserve combined validation/formatting baseline report. |
| `efcae0bb0164` | H | Preserve completed task records and remaining-gap history. |
| `9c3e21a2a731` | H | Preserve maintenance resource-bound limitation; do not claim fixed without evidence. |
| `79b8c02cc5c1` | H | Preserve actual native-client validation record. |
| `37cea9182644` | A | Delete IndexStorageStatus; use live status/readiness per D3. |
| `ea54cc661a3c` | A | Delete TUI storage controls; retain live cancellation/shutdown and source check behavior. |
| `c391d1119fec` | A | Delete desktop storage views/controls; retain replacement-open and worker guards. |
| `0346ccfdda74` | H | Preserve frontend validation/completion record. |
| `60979c693753` | H | Preserve frontend integration/cleanup record. |
| `6ddde6e7e202` | A | K07: keep surviving cfg(test)/dead-code cleanup; no warning suppression. |
| `2b8565187449` | H | Preserve compile-warning report. |
| `dd3f8feb19df` | H | Preserve warning cleanup integration record. |
| `bf34e80dbe4e` | H | Preserve direct-access baseline archive. |
| `01f83cf8f79b` | D | Delete persisted gram presence and reader helpers; keep native key-count behavior if used. |
| `3e37e57f198c` | A | Preserve paired-query evidence; runnable campaign uses native memory mode. |
| `27fce88fac5c` | H | Preserve historical direct-access work inventory. |
| `ea79a6f81678` | D | Delete runnable managed-open allocation benchmark; retain archived evidence. |
| `8fd0a71d99e2` | A | Retain alternating fresh-build comparison methodology; remove saved-open scenarios. |
| `6b6677cf12fe` | D | Retire cold saved-index opening harness; retain archived evidence. |
| `758c9ef9feb0` | D | Delete direct encoded record slot tables. |
| `111e78205c96` | D | Delete byte-backed record table reader. |
| `87abc5bf18ed` | H | Historical format doc; mark retired/remove from active docs. |
| `21dbcf1b8ccb` | D | Delete offset-indexed generation catalog reader. |
| `b3374194405e` | M | Direct catalog/segment integration. |
| `6e7c0216a32a` | A | Delete byte decoder and persisted dispatch; restore native borrowed metadata accessors. |
| `f19d96f026b9` | M | Direct reader milestone integration. |
| `19fb4d998bb0` | A | Convert consumers back to native references; retain newer query algorithms from K08–K17. |
| `61c430689156` | D | Delete persisted bounded file-entry accessors; native vectors already own entries. |
| `8617be025328` | A | Native metadata bounds with existing semantic validation/errors. |
| `0b7d04918a9a` | A | Native posting-value enumeration; preserve bounded cursors and selection identity. |
| `b316f543b7b4` | D | Delete retained reader for maintenance reencoding. |
| `4c7a46105beb` | A | Restore native compaction metadata reads; keep K18–K24 algorithms. |
| `ffb8daeaaa53` | M | Direct segment consumer integration. |
| `6edb0349aac0` | A | K17: retain 64-KiB scheduling guard using cached native maximum range. |
| `3ecab86ee520` | D | Delete obsolete-manifest retirement. |
| `78eb4dd8f850` | A | Retire reopen premise; retain first-update workload on fresh native index and matched provenance. |
| `ec03f9e56b55` | D | Retire initial managed flush runner. |
| `a2bcff042cf1` | A | Delete persisted catalog path lookup/base; retain native bounded overlay/shard ownership. |
| `4a7a2f1b43a6` | A | Preserve pinned mutation/catalog oracle assertions with native construction. |
| `8a29395e046d` | M | Catalog/segment integration. |
| `d8d67f8ee875` | M | Catalog/segment integration. |
| `450623e41862` | M | Catalog/segment integration. |
| `84c5bca8646c` | M | Catalog/segment integration. |
| `a7a62297c4ea` | D | Delete direct managed catalog installation/history references. |
| `8efa9aff1ce0` | M | Catalog/integration merge. |
| `5fe3c6bdce7f` | A | Native lifecycle maintenance access; preserve cancellation/publication checks. |
| `02f9ed24910e` | M | Catalog/segment merge. |
| `d48039c29b30` | MR | Fallible cheapest_consolidation_pair resolution served byte readers. Restore native Option result, native overlap checks and bounded pair choice (D4). |
| `c0f529357568` | D | Delete obsolete-format disk-open rebuild logic. |
| `e8e809d42781` | M | Persisted catalog integration. |
| `966c5bfc39f4` | D | Delete persisted row validation; preserve native segment invariants. |
| `efda6ac81fd0` | M | Direct validation integration. |
| `517e6a7b736e` | D | Remove managed-base setup from runnable campaign; preserve query cases. |
| `a35fb9ee3f0d` | A | K14: preserve metadata fast-path guards; native scalar metadata. |
| `6acaf8af08a6` | D | Delete persisted source-revision field; native FileEntry already owns revision. |
| `9719d4f6d19d` | D | Delete checked byte span reuse. |
| `e2a09b1d3f18` | A | K15: sparse candidate-file relation scans; native revision checks. |
| `cdcafb1e0652` | D | Delete persisted membership span reader; borrow native file records. |
| `84bd746cff88` | A | K16: defer full record/result/catalog work until page admission. |
| `42a4bfafd0be` | H | Preserve final direct-access validation inventory. |
| `8747efc47ea4` | K | K14: retain direct indexed timestamp comparison and eligibility guards. |
| `175c94bc1519` | D | Delete direct snapshot-open phase instrumentation. |
| `4d8461cfc98c` | H | Preserve direct-access measurements/report. |
| `0c6365145822` | H | Preserve direct-access completion record. |
| `1e22afd17bec` | K | K01: binding cache, budget, hit metrics and retained-size helpers stay deleted. |
| `e5d6f250bdc7` | K | K03: preserve configuration hashing byte-for-byte per D1. |
| `edb1e753db7b` | K | K02: retain fresh-binding diagnostics and parity evidence. |
| `560d83602311` | H | Preserve binding-cache removal measurements/report. |
| `9cdb94cf68f2` | H | Preserve binding-cache removal completion record. |
| `eb98d7ef18d7` | M | Binding-cache removal integration. |
| `da608b2799a6` | A | K04/K28: legacy APIs stay deleted; migrate managed test replacements to native tests. |
| `dbe113a3e38e` | H | Preserve legacy-removal report and issue triage/moves. |
| `e64a5caad599` | M | Legacy-removal integration. |
| `4d7d5b61bf27` | D | Delete explicit try_load APIs and read-only managed open. |
| `05828120b803` | D | Delete flush_to and destination-generation reports/state. |
| `31b0f67e46c4` | A | Remove CLI load/flush; retain progressive source construction and ordered query transport. |
| `89163825efab` | A | Remove TUI open/save/retry; retain source open, queries, checks and maintenance. |
| `6b78ba6cd632` | A | Remove desktop load/save; retain session replacement/cancellation/shutdown. |
| `8e2189e1eb9e` | D | Delete desktop disk target configuration. |
| `b54822f9afdf` | A | Old combined flags stay absent; also delete replacement load/flush flags. |
| `1df5f2627491` | A | Remove persistence end-to-end; keep source exclusions, ordinary errors, benchmark provenance and final source-opening behavior. |
| `a0a0691897c6` | H | Preserve explicit persistence completion record as history. |

## Execution evidence columns

The implementing agent must append final commit/file/test evidence for every K/A row in its task report or task memory. D rows are closed by the deleted-module/API audits plus migrated semantic tests, H rows by a history-preservation diff, and M/MR rows by the parent/resolution dispositions above.

This ledger covers only B..H. If the execution base differs, enumerate H..START (including side branches and merge resolutions) before editing. Do not presume an uninspected TOML implementation is absent from a newer base.

## H..START completion (review follow-up, 2026-11-14)

The original ledger audited `B..H`. The actual execution base is `START = 66b932f31f922a13062a61658e843e3033b3aff4`. This appendix records the six commits in `H..START` (`git log --reverse --topo-order H..START`) and their final-state disposition. No commit was replayed; the restoration started from the merged START tree and removed only persistence behavior. `git show --remerge-diff 66b932f31f` produced no additional resolution.

| Commit | Code | Final-state disposition |
| --- | --- | --- |
| `b31db7b2eb97` Replace binary index manifest with readable TOML generations | D/A | Removed the TOML manifest, generation directories, segment descriptors, catalog envelopes, write-validation/stamp rules and hard-link reuse. Retained the source-exclusion rule for `.objekte/.indexes` and `.objekte/indexes` in `repository_configuration::prepare_source_definitions`. The live configuration hash keeps the START domain value (`CONFIGURATION_HASH_DOMAIN = 1`); `MODEL_VERSION` was no longer part of START's live hash, so its removed test is not restored. Native segment metadata replaces descriptor readers. |
| `f4337dc5c2d0` Report TOML manifest performance and fragmented-save bug | H | Retained unchanged: `docs/ai/generated/toml_index_manifest_performance.md` and `docs/ai/issues/bug/fragmented-loaded-save-identity-mismatch.md`. The save-identity issue is now obsolete with persistence removal and is listed in the completion report. |
| `e6f5fe8f25b6` Complete the TOML index manifest task | H | Retained unchanged as `docs/ai/done/toml-index-manifest.md`. Its implementation is superseded by the memory-only restoration; its measurements remain historical evidence. |
| `7e32a5a7674e` List exact load benchmark commands | H | Retained unchanged in the generated TOML performance report. Those commands are historical and are not current runnable storage gates. |
| `31d0f3c6a23e` Correct final core test count in completion report | H | Retained unchanged in the completed TOML task report. |
| `66b932f31f92` Merge TOML index manifest into master | M | Clean integration merge with parents `a0a0691897` and `31d0f3c6a2`; no nonempty remerge diff. The merged tree is `START`; all six dispositions above apply to it. |

The `b31…` source-exclusion ordering and spelling are preserved so that prepared definitions and configuration hashes for unchanged inputs remain byte-identical to START. Generated namespaces are additionally enforced consistently across discovery, mutation, scan and alias entry points in the review follow-up; those checks do not change legitimate prepared definitions or hashes.
