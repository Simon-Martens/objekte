# Objekte

Objekte is a local JSON search workspace with a reusable Rust core, a terminal interface, and the `obj` command for schema-free folder search.

- [Use the `obj` command](docs/cli/obj.md) to search a directory of JSON documents and stream complete results as NDJSON.
- [Development code guidelines](docs/development/code-guidelines.md) describe the Rust workspace conventions.
- [Posting maps](docs/development/posting-map.md) describe how index postings are stored and when the persistent backing is used.

```sh
cargo build --release -p objekte-cli
./target/release/obj --path ./documents --fts Goethe
```
