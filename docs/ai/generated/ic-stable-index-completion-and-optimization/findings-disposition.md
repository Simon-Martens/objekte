# Findings disposition — completion and optimization

Task: `ic-stable-index-completion-and-optimization`  
Reviewed repair: `1022768be3d625be7acec28da38defd831b85fe1`

| Finding | Disposition | Evidence / remaining work |
| --- | --- | --- |
| Lifecycle used empty facts | **Fixed** | `stable_index::prepare_file_change` uses `process_batch` on one file and emits typed/length/state/gram/sort/composite/relation facts. Lifecycle benchmark uses it, changes content at replacement 5, and checks stable matches against native reconstruction (`semantic_matches=20`). |
| Mutation failure boundary | **Fixed for append/replace/delete/move/posting** | `application_started` tracking plus wrappers marks first failure after the first write; prevalidation errors remain recoverable. Fault tests cover append/delete/move after first write and invalid preparation. |
| Replacement destination collision | **Fixed** | Validation rejects a destination occupied by another file before changing either path. Regression covers both files preserved. |
| Prepared component/key-role validation | **Fixed** | Validation checks collection ownership, component kind/key tag, local slots, relation metadata, duplicate collection entries and coverage components before application. Wrong-kind test does not panic. |
| Composite rebasing | **Fixed** | `shift_component_key` handles `CompositeSortEntryKey.slot` with checked arithmetic. Regression changes a composite file at a nonzero base and verifies order after save/reopen. |
| R02 owned conversion | **Not complete** | `CollectionState` is still borrowed by `from_collections`; no `OwnedComponentInput` job graph. Peak/resident reflect native/stable overlap. |
| R03 bounded waves / all owners | **Partial** | Posting fields merge in parallel inside `admit_parts`; metadata admission is separated. Custom postings, packed bigrams, sort-key and relation owners still merge serially; explicit wave barrier absent. |
| R18 counters / current memory | **Partial** | Observed insert/bitmap counters and memory length/capacity records exist. Some unavailable diagnostics still zero; loaded component live capacity inspection not implemented. |
| R19 query distributions | **Partial** | Candidate point lookups use 200 repetitions and median/p95. Native matched distributions for point/range/length/state/FTS/sort/relation are missing. |
| Composite initial construction | **Fixed** | Composite components use native constituent comparison/directions/tie rules and a dedicated component. |
| Composite mutation/reopen | **Fixed** | Test covers changed composite, nonzero base, filtered dead slot, order and reopen. |
| Shared/empty-file lifecycle | **Partially retained** | Existing tests cover shared catalogs and zero-record paths/deletion; unequal per-collection counts and all empty/nonempty replacement combinations still need explicit completion coverage. |
| Relation lifecycle | **Partial** | Prepared relation facts are supported by the file-preparation path; target addition/deletion, duplicate occurrence and source-liveness matrix remain incomplete. |
| O01 sorted insertion | **Rejected** | 3 paired P=8: current 3.100 s, sorted-insert candidate 3.144 s (-1.4%); sort job sum 4.087→4.691 s. Reverted. |
| O02 catalog/copy cleanup | **Rejected/inconclusive** | Dense path grouping showed +2.3% in one 3-pair set but -2% in combined/final sets. Reverted to the simpler proven path grouping; cached high-water and checked FileId exhaustion retained. |
| O03 buffer capacity | **Rejected** | Shrink-to-fit did not reduce reported capacity in the measured path, added ~2% time, and did not materially improve HWM. Disabled by default. |
| O04 cached sort-key bytes | **Not run** | No profile evidence justified it; no speculative code retained. |
| Measurement aggregation | **Fixed** | Campaign driver now aggregates medians/ranges for stages, RSS and peak; selected trace is explicitly one sample. |

## Remaining mandatory work

1. Implement owned `ComponentBuildJob` inputs and release native posting/text/
   sort owners as each stable job finishes (R02).
2. Implement the bounded waves and move all owner-family merges outside the
   coordinator (R03).
3. Add matched native/stable primitive distributions for every required family
   (R19).
4. Finish live-current memory reporting and null unavailable diagnostics (R18).
5. Complete the shared/empty/relation lifecycle matrix.
