# Typed ID representation

Declared `LogicalType::Id` values are converted from source strings to `Value::Id(String)` before posting construction. The source value must satisfy the existing intrinsic record-ID rule: nonempty and outside the reserved generated/fallback prefixes. Query literals retain their spelling, so `@record.id` can still find a generated record identity. IDs are case-sensitive and support equality and inequality only. `@record.id:lower` remains an explicit string view and retains its case-folded behavior.

`ScalarKey::Id(String)` remains distinct from `ScalarKey::String(String)`, including in posting-value bounds and counts. Optional ID sort columns order canonical ID strings exactly by Unicode scalar/UTF-8 lexicographic order, without ICU case folding. String sort columns continue to use configured ICU collation.

Reader-declared logical string coverage continues to include source strings converted to IDs. Unscoped FTS and explicit metadata-ID FTS can search their source spelling; this does not permit FILTER substring operators on logical IDs or dereference an ID without a relation declaration. The logical payload retains the typed value, while returned source bytes remain original.

The new logical and posting variants change the encoded snapshot representation. `MODEL_VERSION` advanced from 5 to 6 and `SNAPSHOT_VERSION` from 6 to 7 in the implementation commit. Old snapshots follow the existing incompatibility/rebuild path. Repository-local prepared and verified caches are not persisted.
