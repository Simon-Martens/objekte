# JSON and schema validation

Original JSON files are authoritative. Sonic parses records when the cache is updated or a candidate is read. JSON Schema validates records when they are returned to a caller or when `validate` is requested. The segment cache stores no persistent validity state, diagnostics, or pending validation queue.

- `Repository::get_record` returns the current file, its JSON/schema status, and diagnostics. Malformed JSON is returned with `incorrect_json`, a null parsed value, and the raw bytes.
- `Repository::open_view` validates returned page records. Filtering and sorting operate on parsed values; a malformed record cannot satisfy a JSON filter.
- `Repository::query_json` validates returned matches. A parseable record can match a declared field even if another field violates its schema. Malformed JSON never matches.
- `Repository::validate_collection` and `validate_all` scan and validate the requested source files explicitly. They do not publish cache validation states.

The cache's scalar lists encode value, null, missing, and invalid-type positions for sorting and filtering. Those keys do not declare whether a JSON document is valid. Source records can be edited externally; reconciliation updates cache metadata and text, while validation remains tied to the bytes read for a particular request.

Reference occurrences are also derived on request. `references_from` extracts declared edges from one source record. `references_to` scans all registered referring collections, so changes and deletions are seen without persisted edge rows. `--check-targets` checks whether target source files currently exist.

```sh
cargo run -p objekte-cli -- examples/musenalm validate contents
cargo run -p objekte-cli -- examples/musenalm get contents RECORD_ID
cargo run -p objekte-cli -- examples/musenalm cache status
```
