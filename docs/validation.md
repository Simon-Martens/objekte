# JSON and schema validation

For the default provider, original JSON files are authoritative and Sonic parses them when an index is built or refreshed and when a candidate is read. A registered provider instead supplies authoritative source bytes and a decoder supplies the logical record validated against any bound JSON Schema. A custom decoder error is returned as an error; JSON's malformed-file fallback is specific to the built-in format. When a collection has a schema, validation is on demand; the in-memory indexes store no validity state, diagnostics, or a pending validation queue. `pending` means a logical record was decoded but has not yet been checked against its schema for that response. `no_schema` means the collection has no schema to validate against.

- `Repository::get_record` reads the selected source and returns its logical record. A decoded record has `pending` status with a schema or `no_schema` without one. Built-in malformed JSON is returned with `incorrect_json`, a null parsed value, and the raw bytes; a registered decoder may instead fail according to its own contract.
- `Repository::open_view` validates returned page records in memory mode and on the selected-source route. Native direct mode returns parseable page records with `pending` schema status. Filtering and sorting operate on logical values; malformed JSON cannot satisfy a view filter.
- `Repository::query_json` validates decoded returned matches. A filter using `@status` validates every decoded candidate before comparing its status. A decoded record can match a logical field even if it violates its schema or the field is undeclared. Built-in malformed JSON has no JSON field paths, but can match source metadata predicates such as `@status = "incorrect_json"`. Standalone word and regex search can also return malformed JSON with a syntax issue and raw source bytes.
- Named views execute these same JSON or full-text query APIs, so their returned records follow the same validation rules.
- `Repository::validate_collection` and `validate_all` scan and validate schema-backed collections explicitly. A schema-free collection reports zero validations. These calls do not publish cache validation states.

The native cache's scalar lists encode value, null, missing, and invalid-type positions for sorting and filtering. Those keys do not declare whether a record satisfies its schema. Source records can be edited externally; reconciliation updates indexes and metadata, while validation remains tied to the bytes read for a particular request.

Every returned record includes its repository-relative path, source byte size, and exact signed modification time as a decimal Unix-nanosecond string. Creation time is not returned. The separate process-local revision timestamp records when a change was observed, not the filesystem modification time.

Reference occurrences are also derived on request. `references_from` extracts declared edges from one source record. `references_to` reports the currently observed edges unless `--complete` requests a scan of all referring collections. `--check-targets` checks whether target source files currently exist. There are no persisted edge rows.

```sh
cargo run --release -p objekte-cli -- examples/musenalm validate contents
cargo run --release -p objekte-cli -- examples/musenalm get contents 00a2tiwxye101sh
cargo run --release -p objekte-cli -- examples/musenalm cache status
```
