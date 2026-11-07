# Dynamic undeclared FILTER candidates

## Semantics and coverage

An undeclared positive string equality/contains predicate may use the ordinary shared raw bigram store when the reader declares complete logical-string coverage, the raw representation is identity text, the operator is a built-in nonfallible literal comparison, and the folded request has usable grams. The field remains dynamically bound as `Unknown`; record-level gram membership is only a necessary condition. Exact field/path/quantifier evaluation remains the residual. Positional selectors can use this record-wide necessary condition, but typed postings and field-only custom grams retain their existing positional restrictions.

For a multivalued default universal predicate, `G ∪ E` is the candidate superset: `G` is the gram membership and `E` is the collection-wide logical-record bitmap for any empty array. The selector returns a successful empty selection only when it encounters an empty array during implicit or wildcard traversal. An explicit out-of-range array index is missing. A nonempty array of objects with an absent child remains missing. The evaluator can make a present empty selection true only for a multivalued universal comparison, so scalar/explicit-position and existential predicates omit `E`. Unrelated empty arrays deliberately overinclude records; the residual rejects them.

Every successfully prepared logical record is observed after schema conversion. The built-in logical-string derivation records `E` in the existing raw text walk; reader-owned segment derivations use a direct walk of the already parsed logical value. Raw fallback records never enter `E`. Batch-local `E` slots are shifted on deterministic admission. The published state validates `E ⊆ logical_records`; the restored state checks the same domain invariant. Snapshot format version 10 rejects old payloads, and the existing section checksum protects the serialized bitmap. Complete E coverage follows from visiting every logical record during construction and rejecting old layouts; no sample can authorize this restriction.

EXPLAIN records dynamic binding, raw representation/reader version, gram count, structural count, selected candidate count, and retained residual. It reports incompatible reader/raw coverage or no usable grams when scanning is selected. A broad `E` is costed as the actual `G ∪ E` cardinality and can cause Scanner selection.

## Focused evidence

- Fixed expected IDs cover scalar, array, number, null, absent, terminal/ancestor/unrelated empty arrays, false universal witnesses, nested wildcard and positional paths, Unicode case folding, metadata OR with malformed fallback, AND/OR/NOT, parameters, and both alphabets. Optimized hits/count/evidence agree with the independent reference route, fresh and restored, with warm verified cache.
- A selective unknown-field query selects RawBigram and reads/parses fewer source records than forced reference scanning with inference disabled. The broad empty-array fixture selects Scanner for universal comparison and RawBigram for existential comparison.
- A reader omitting title from its raw segments forces Scanner; its otherwise identical complete-coverage registration permits RawBigram. Both return the exact title match.
- The 512-file batch fixture puts empty arrays on opposite sides of the batch boundary and confirms equal shifted bitmaps for one-worker, parallel and restored states. Checksummed snapshot corruption that marks a fallback or out-of-range slot as empty-array is rejected.
- `cargo test --workspace --all-targets` and `cargo clippy --workspace --all-targets` passed after the final code and test change. Clippy reported 79 advisory warnings already present in the foundation baseline, with no additional warnings from this change. The repository-wide spacing check still reports pre-existing formatting drift in untouched parts of the workspace; new files and changed hunks were formatted without committing the unrelated drift.

No full Musenalm benchmark was run on this isolated worker branch. The integration supervisor owns final cross-feature and timing evidence.
