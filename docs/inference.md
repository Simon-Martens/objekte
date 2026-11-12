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

`Disabled` does no inference observation or typed-index construction.
`ReportOnly` observes already parsed logical records and reports field paths
without publishing inferred postings. `AutoIndex` can publish Boolean, numeric
and ordered short-string postings under the default versioned policy. A
separate low-cardinality string route applies when configured string bounds
leave values outside the short-string limit. Declared fields keep their
explicit contracts:
`indexed: true` uses the declared posting and `indexed: false` opts out of
automatic indexing. Inference does not build sort columns during opening;
eligible complete inferred scalar postings can supply a lazy order at query
time. It does not create relations.

Staged opening publishes the validated text-search base before optional field
work. With inference disabled, compact field states are built only for fields
selected by the index configuration; an unconfigured collection has no second
pass. An explicit `AutoIndex` request discovers eligible nested paths and
builds their compact type, presence, and special-value states during the
second source pass. A later explicit inference request can extend an already
searchable session without rebuilding its unchanged bigram index. A field
whose optional posting or state has no coverage remains queryable through
source evaluation. `SearchReady` therefore means correct queries can execute,
not that all optional acceleration is complete.

The report counts logical eligible and raw-fallback unavailable records
separately. `present_records` follows the query selector, including an empty
ancestor array that makes a discovered descendant path present with an empty
selection. `null_occurrence_records` counts records containing a null at the
path; `exact_null_records` counts the narrower selection that makes `:null`
true. `value_occurrences` counts non-null scalar occurrences, while posting
membership is deduplicated per record. A field's `complete: false` marks a
bounded distinct-string summary or structural observation that could not be
completed. The report exposes `short_string_covered_records` and
`short_string_unresolved_records`; the latter still need exact verification.
Aggregate `observed_paths`, `admitted_paths`, `dropped_paths`,
`summary_bytes_estimate` and `index_bytes_estimate` describe construction
without a second record scan.

The default policy limits observed paths to 4,096, depth to 16 and
contributions per record to 4,096. It has two string routes:

- The ordered short-string dictionary accepts values of at most **64 decoded
  UTF-8 bytes**, inclusive. Its retained key and membership estimate is capped
  at **24 MiB per path**. High-cardinality strings can qualify without the
  repetition heuristic. If longer or otherwise unresolved values occur, its
  coverage is partial; searches include those records for exact verification.
  A partial dictionary cannot prove a complete order or facet.
- The separate low-cardinality route matters when a custom policy permits
  general strings longer than its short-string limit. Its default general
  string bound is also 64 bytes, so the ordered short-string route handles
  eligible strings under the default policy. The low-cardinality route retains
  at most **8 KiB of distinct nonblank text** and requires at least 20
  nonblank occurrences from 10 records, at most 64 distinct nonblank values,
  a distinct count at most `max(4, occurrences/10)`, and at least 80%
  repeated occurrences.

These are index-admission rules, not restrictions on valid logical values or
enums. Mixed or unsupported shapes and fields exceeding an optional-index
budget retain exact query behavior through verification or scanning.

Retained inference-state accounting defaults to a **16 MiB per-batch** estimate,
a **512 MiB coordinator** allowance shared evenly among collections, and a
**768 MiB repository** allowance. The 256 MiB difference between repository
and coordinator allowances permits at most 16 admitted 16 MiB batches at once,
even if more workers are configured. A batch's content determines whether it
crosses its own limit; worker count does not change that decision. These are
inference-owned estimates for path summaries and posting contributions. They
are not process RSS limits and do not cover ordinary indexes or source
buffers. A budget breach marks observation incomplete and prevents publishing
partial inferred postings.

For declared manual indexes and explicit opt-outs, observation skips
provisional postings. It keeps only bounded distinct-string summaries for
reporting, subject to the distinct-count and distinct-byte limits, and marks
truncated summaries incomplete. `ReportOnly` likewise reports observations
without publishing inferred postings.

Inference mode and policy participate in snapshot compatibility. Reopening
with a different mode or policy may rebuild from source; a `Disabled` opening
does not activate `AutoIndex` postings from a previous snapshot. Queries keep
their original membership, evidence, payload and error semantics across modes.
A complete compatible scalar posting can prove supported comparisons without
source reads. Partial short-string postings add covered candidates while
unresolved records remain eligible for verification. Quantifiers and selectors
retain their existing residual checks. Complete presence and exact-null
postings can prove their existing derived predicates.

The runnable folder demonstration accepts `--inference disabled|report|auto`:

```sh
cargo run --release -p objekte-search-core --example musenalm_without_schema -- \
  --root examples/musenalm --folder places=data/places --query Berlin --inference report
```

The logical value model accepts JSON null, Boolean, number, string, object and
array roots, including mixed nested arrays. Valid data need not meet an
inferred-index admission rule. Malformed records use the existing diagnosed
raw-fallback route. Inference observes logical values from registered readers;
it does not infer raw-text coverage from those values.
