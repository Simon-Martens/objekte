# Assignment 2 JSON-use inventory

Command:

```sh
rg -n 'serde_json|SourceFormat::Json|source::json' crates/core/src
```

| Owner | Representative path | Input and responsibility |
|---|---|---|
| Source JSON implementation | `source/json/document.rs`, `source/json/scalars/mod.rs` | Record bytes and framed ranges; this is the only production record-byte JSON decoder reached after reader resolution. |
| Model conversion | `model/json.rs` | Already parsed `serde_json::Value`/`Number` values converted into `Value`/`Number`, including exact-decimal fallback. `Number::parse_json_literal` separately consumes numeric token text from the query parser. |
| Schema documents | `schema/json/mod.rs` | Schema file bytes compiled into a `Schema`. |
| Repository configuration | `repository/configuration/loader.rs` | Collection configuration documents, including format and identity names. |
| Typed query parameters | `query/parameters.rs` | JSON scalar or tagged parameter transport converted into logical values. |
| Workload manifests/reports | `workload.rs` | Fixed and extended benchmark manifests and typed report data. |
| Field-path string escaping | `model/mod.rs` | Quoted JSON strings are rendered and decoded for field-path segments containing punctuation. |
| Archived diagnostic JSON | `measurement/mod.rs` | Test-only `WorkCounters` compatibility coverage for a retired persisted key. |

The production `serde_json::from_slice`, `from_str`, and
`Deserializer::from_slice` hits outside `source/json` consume schema,
configuration, parameter, or workload text rather than record bytes. No
remaining record-byte JSON decoder was found outside the resolved source
implementation. Registration names such as `Registered("json")` select a
reader but do not grant relative-path identity eligibility. In the built-in
registry, `Registered("json")` resolves to `JsonReader`,
`scalar_verification() == Some(ResolvedStrings)`, and retains generated folder
identity. In a fresh registry, a custom implementation registered under the ID
`json` can opt into trusted query extraction, but its extension provenance
still makes `scalar_verification() == None`; declared `Registered("json")`
therefore remains ineligible for explicit relative-path identity.

Declared `Json` is a separate case: it retains relative-path eligibility even
when a fresh registry resolves the ID `json` to a custom implementation.
Multiple logical records in one file then produce the same relative path ID,
and indexed construction rejects the duplicate identity.
