# Musenalm Objekte–Tantivy comparison

Run from the repository root with the seven collection fixture available at the same physical path for both engines.

```sh
python3 bench/run_musenalm_tantivy.py quick --root examples/musenalm --result-dir bench/results/musenalm/tantivy-quick
python3 bench/run_musenalm_tantivy.py full --root examples/musenalm --result-dir bench/results/musenalm/tantivy-full
```

The result directory must be fresh. `--cases Q001,Q044`, `--query-workers N`, `--build-workers N` and `--repetitions N` override the defaults. The full command inventories all 220 source cases, verifies every admitted count against complete identities, checks supported locator pages, warms each request once and records seven release samples per distinct effective request and contract. The quick selection includes Q116 to demonstrate a visible unsupported case. The result directory contains `run-manifest.json`, `compatibility-inventory.json`, `verification.json`, both engines' raw samples, `summary.json` and `REPORT.md`.

The comparison uses Tantivy 0.27.0 in a dedicated workspace member outside the default members. The binary uses the public Objekte API, historical and extended Musenalm schema loaders and bound benchmark requests. The normal core, CLI and desktop dependencies remain separate from Tantivy.

## Index definitions

Objekte uses the ordinary in-memory index and the historical Musenalm schema for the first 150 cases. The extended loader is used for later cases when binding and verification require relation annotations. Its default ASCII bigram alphabet, source-backed fields, native postings and sort columns are retained. Verified-result cache bytes are zero. The collection definitions are loaded from `.objekte/collections/*.schema.json` and the existing Musenalm registration.

Tantivy uses a RAM directory. Each source JSON file is one document with stored collection, record ID, relative path and byte end. `source` is a stored, indexed and fast JSON field with the raw tokenizer, preserving typed booleans and numbers and exact unanalysed strings. The collection and record ID are exact text terms. `identity_ordinal` is a fast field assigned in ascending `(collection, record_id)` order. `numbering_sort` is a fast floating-point field for bounded numeric page collection. The general derived `fts_global` and `fts_scoped` fields contain raw, case-sensitive text terms from every decoded string value. String values are split into overlapping 1024-scalar chunks beginning every 512 scalars. Literal needles up to 512 scalars are matched with a regex automaton over these term dictionaries, with dot-all newline handling and literal escaping. No query-specific answer field is stored. The Tantivy index materializes paths, types, numbering and all strings that Objekte can read from source. Its build time, managed segment byte size and process RSS include this preparation.

The inventory records the original bound request, count and locator-page projections, separate statuses and reasons, representation and measurement aliases. Direct field terms and ranges use the typed JSON field. Literal FTS uses the derived text fields. Count requests use Tantivy's `Count` collector. Pages use a single search with `Count` and a bounded ordered collector, followed by retrieval of winning stored locators inside the timer. Explicit text and compound sorts, complex string operators, metadata facts, relation traversals and other unsupported mappings have separate inventory reasons. A count remains eligible when its page sort is unsupported. Q137–Q139 are locator projections of payload/evidence requests. Their timings describe locators only.

The verifier resolves every Tantivy hit to its stored identity, rejects duplicates and compares the complete identity set to Objekte. Supported pages compare ordered `(collection, record_id, relative_path, byte_start, byte_end)` tuples. The independent historical `jq` oracle is run separately with `bench/verify_jq_oracle.py`, and its corpus metadata fingerprint must match the comparison manifest before its digests are reused.
