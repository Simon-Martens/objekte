# Full Musenalm page and payload acceptance

Search core revision `128a7601c2`. All 150 fixed queries passed against the index-free scanner under ASCII, and all 150 passed under Unicode: **300/300**.

The comparison checks exact counts, ordered page identities, locators, file metadata, matched-field evidence, and byte/logical payloads. The scanner shares the core's query binder and parser, so the separate [raw-file jq oracle](../jq-oracle-128a7601c2/REPORT.md) supplies independent predicate semantics.

Reproduce with `cargo run --release --example accept_musenalm -- --alphabet ascii` and repeat with `--alphabet unicode`.

Logs: [ASCII](ascii.log), [Unicode](unicode.log).
