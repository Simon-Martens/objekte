# JSON and schema validation

Original JSON files are authoritative. Sonic parses records when an index is built or refreshed and when a candidate is read. When a collection has a schema, validation is on demand; the in-memory picker stores no validity state, diagnostics, or a pending validation queue. `pending` means JSON parsing succeeded but the schema has not yet been checked for that response. `no_schema` means the collection has no schema to validate against.

- `Repository::get_record` returns the current file. A parseable record has `pending` status with a schema or `no_schema` without one. Malformed JSON is returned with `incorrect_json`, a null parsed value, and the raw bytes.
- `Repository::open_view` validates returned page records in memory mode. Direct mode returns parseable page records with `pending` schema status. Filtering and sorting operate on parsed values; malformed JSON cannot satisfy a view filter.
- `Repository::query_json` validates parseable returned matches. A parseable record can match any stored field even if it violates its schema or the field is undeclared. Malformed JSON cannot match a field-based filter, including `:fts`. Standalone word and regex search can return a malformed file with `incorrect_json`, a syntax issue, and raw source bytes.
- `Repository::validate_collection` and `validate_all` scan and validate schema-backed collections explicitly. A schema-free collection reports zero validations. These calls do not publish cache validation states.

The cache's scalar lists encode value, null, missing, and invalid-type positions for sorting and filtering. Those keys do not declare whether a JSON document is valid. Source records can be edited externally; reconciliation updates indexes and metadata, while validation remains tied to the bytes read for a particular request.

Every returned record includes its repository-relative path, source byte size, and exact signed modification time as a decimal Unix-nanosecond string. Creation time is not returned. The separate process-local revision timestamp records when a change was observed, not the filesystem modification time.

Reference occurrences are also derived on request. `references_from` extracts declared edges from one source record. `references_to` reports the currently observed edges unless `--complete` requests a scan of all referring collections. `--check-targets` checks whether target source files currently exist. There are no persisted edge rows.

```sh
cargo run --release -p objekte-cli -- examples/musenalm validate contents
cargo run --release -p objekte-cli -- examples/musenalm get contents 00a2tiwxye101sh
cargo run --release -p objekte-cli -- examples/musenalm cache status
```
