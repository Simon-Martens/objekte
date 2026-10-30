# JSON and schema validation

Original JSON files are authoritative. Sonic parses records when an index is built or refreshed and when a candidate is read. Schema validation is on demand; the in-memory picker stores no validity state, diagnostics, or a pending validation queue. `pending` means JSON parsing succeeded but the schema has not yet been checked for that response.

- `Repository::get_record` returns the current file. A parseable record has `pending` schema status. Malformed JSON is returned with `incorrect_json`, a null parsed value, and the raw bytes.
- `Repository::open_view` validates returned page records in memory mode. Direct mode returns parseable page records with `pending` schema status. Filtering and sorting operate on parsed values; malformed JSON cannot satisfy a JSON filter.
- `Repository::query_json` validates returned matches. A parseable record can match a declared field even if another field violates its schema. Malformed JSON never matches.
- `Repository::validate_collection` and `validate_all` scan and validate the requested source files explicitly. They do not publish cache validation states.

The cache's scalar lists encode value, null, missing, and invalid-type positions for sorting and filtering. Those keys do not declare whether a JSON document is valid. Source records can be edited externally; reconciliation updates indexes and metadata, while validation remains tied to the bytes read for a particular request.

Reference occurrences are also derived on request. `references_from` extracts declared edges from one source record. `references_to` reports the currently observed edges unless `--complete` requests a scan of all referring collections. `--check-targets` checks whether target source files currently exist. There are no persisted edge rows.

```sh
cargo run -p objekte-cli -- examples/musenalm validate contents
cargo run -p objekte-cli -- examples/musenalm get contents RECORD_ID
cargo run -p objekte-cli -- examples/musenalm cache status
```
