# Optional field inference

`CollectionDefinition::folder` opens a recursive collection with an empty schema and shared text indexing. Field inference is disabled by default. It never edits schema or source files and it does not change how an undeclared query field is bound or compared.

Choose a mode at opening:

```rust
use objekte_search_core::inference::{InferenceConfig, InferenceMode};
use objekte_search_core::repository::{CollectionDefinition, Repository, RepositoryOptions};
use objekte_search_core::source::SourceFormat;

let definition = CollectionDefinition::folder("items", "data/items", SourceFormat::Json);
let options = RepositoryOptions {
    inference: InferenceConfig {
        mode: InferenceMode::AutoIndex,
        ..Default::default()
    },
    ..Default::default()
};
let repository = Repository::open_with_options("/repository", vec![definition], options)?;
let report = repository.inference_report("items");
# Ok::<(), String>(())
```

`Disabled` does no inference observation or typed-index construction. `ReportOnly` observes the already parsed logical records and returns path statistics without publishing inferred postings. `AutoIndex` additionally admits complete Boolean, numeric and repetitive low-cardinality string postings when the versioned policy permits them. Declared fields keep their explicit contracts: `indexed: true` is reused and `indexed: false` is treated as an explicit indexing opt-out. Inference creates no sort column or relation.

The report counts logical eligible and raw-fallback unavailable records separately. `present_records` follows the query selector, including an empty ancestor array that makes a discovered descendant path present with an empty selection. `null_occurrence_records` counts records containing a null at the path; `exact_null_records` counts the narrower selection that makes `:null` true. `value_occurrences` counts non-null scalar occurrences, while posting membership is deduplicated per record. A field's `complete: false` marks a bounded distinct-string summary or structural observation that could not be completed. No incomplete field becomes an exact inferred index. Aggregate `observed_paths`, `admitted_paths`, `dropped_paths`, `summary_bytes_estimate` and `index_bytes_estimate` expose construction outcomes without a second record scan.

The default policy limits paths to 4,096, depth to 16 and per-record contributions to 4,096. A provisional string dictionary admits values of at most 64 UTF-8 bytes and retains at most 8 KiB of distinct text. Its heuristic needs at least 20 nonblank occurrences from 10 records, at most 64 distinct nonblank values, a distinct count at most `max(4, occurrences/10)` and at least 80% repeated occurrences. These are index admission rules, not validation or enum restrictions. Mixed or unsupported shapes and fields exceeding an optional-index budget still use the exact scanner.

Retained inference-state accounting defaults to a 64 MiB repository-wide estimate: the 48 MiB coordinator allowance is split evenly among collections that are being built, and each batch has a fixed 256 KiB cap. The scheduler limits simultaneous batches to the 16 MiB remainder (64 jobs at these defaults), independently of the configured worker count. A batch's admission decision therefore does not change merely because more workers are configured. The estimate covers inference-owned path summaries and posting contributions; it is not a process RSS ceiling and does not include the core's ordinary indexes or source buffers. A budget breach marks the observation incomplete and prevents partial inferred postings from being published.

Inference mode and policy participate in snapshot compatibility. Reopening with a different mode or policy may rebuild from source; a Disabled opening does not activate AutoIndex postings from a previous snapshot. Queries keep their original membership, evidence, payload and error semantics across modes. An inferred posting can restrict a dynamic scalar comparison, with residual verification retained for quantifiers and selectors. Complete presence and exact-null postings can prove their existing derived predicates.

The runnable folder demonstration accepts `--inference disabled|report|auto`:

```sh
cargo run --release -p objekte-search-core --example musenalm_without_schema -- \
  --root examples/musenalm --folder places=data/places --query Berlin --inference report
```

The current model accepts object records and homogeneous arrays. Invalid or unsupported records use the existing diagnosed raw-fallback route. Inference observes logical values from any registered reader; it does not infer raw text coverage from those values.
