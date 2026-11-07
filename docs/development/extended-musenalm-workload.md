# Extended Musenalm query workload

`crates/core/benches/musenalm_queries_extended.json` contains the original 150 requests unchanged, followed by Q151–Q188. The extra requests exercise inline `SORT BY`, typed parameters, field operands, array positions and paired occurrences, temporal periods and frozen macros, byte sizes, literal and regex FTS, case selection, forward and backlink relations, selected relation FTS, multi-hop paths, and mixed requests.

The historical commands keep using the original 150-request manifest and relation-free Musenalm adapter. Pass `--extended` to `bench_musenalm`, `accept_musenalm`, or `dump_musenalm_ids` to use the 188-request manifest and opt-in adapter. This keeps historical query/build recordings comparable while measuring the extra relation indexes separately.

The extended adapter reads the six `x-relation.collection` annotations in the original schemas into generic in-memory `RelationDefinition`s. Annotated strings become logical IDs. A declaration without an explicit reverse alias receives `source__field__path`, such as `contents__entry`. The source JSON and schema files are unchanged. Query parameters are carried as JSON data beside the FILTER/FTS text and bound once before timing; `clock_unix_seconds` freezes macros for reproducible cases.

```sh
cargo build --release -p objekte-search-core --example accept_musenalm --example bench_musenalm --example dump_musenalm_ids
./target/release/examples/accept_musenalm --extended --root examples/musenalm --alphabet ascii
./target/release/examples/accept_musenalm --extended --root examples/musenalm --alphabet unicode
./target/release/examples/bench_musenalm --extended --root examples/musenalm --profiles ascii-off,ascii-warm,unicode-off,unicode-warm --verified-cache-bytes 67108864 --repetitions 5 --query-workers 32 --result-dir bench/results/musenalm/extended-CODE_SHA
python3 bench/verify_extended_musenalm.py --root examples/musenalm --result-dir bench/results/musenalm/extended-CODE_SHA/relation-oracle
```

The independent relation oracle evaluates nine requests directly over original JSON and compares complete collection/record-ID sets in both alphabets. The acceptance example compares optimized and reference results, pages, counts, evidence, and payloads for every case.
