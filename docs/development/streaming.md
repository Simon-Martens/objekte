# Synchronous result streaming

`Repository::search_stream` uses the ordinary query preparation, candidate,
verification and ranking paths. `search_stream_with_parameters` binds typed
parameters once. Both calls freeze the current clock once for the request.

`StreamMode::OrderedRows` appends only verified rows in their final page order.
The identity and compatible stored-column cursors can deliver a page before a
requested `COUNT` finishes. Other ranking paths wait until their order is
known. `StreamMode::PageReplacements` sends complete snapshots of the current
best requested page at bounded work checkpoints. A later snapshot replaces the
previous one, and rows may move or disappear. Both modes apply offset and
limit to the requested page. A zero limit or empty result has no data event;
`Complete` identifies the empty final page with sequence zero.

Every callback runs serially on the caller thread and holds a borrowed event
only for its duration. It can return `Continue` or `Cancel`. A shared
`CancellationToken` also stops work between batches and records, including
cached-page materialization and related-target prefetch. One blocking file
read or third-party parser invocation cannot be interrupted midway. Callback
errors, execution errors and resource errors are distinct. An earlier event
does not make a whole query successful: consumers should commit their final
view only after `StreamOutcome::Complete`. No new verified membership cache
entry is published on cancellation or failure.

`StreamProgress::processed_candidates` counts completed source record
verifications plus index-proven record decisions; index-rejected candidates
are excluded. `observed_matches` is a verified lower bound until completion.
`emitted_rows` counts row deliveries, including repeated rows in replacement
snapshots. The configured event and retained-byte limits may return a resource
error for an oversized source record or page. The byte estimate covers known
page winners, sort keys, related target values, in-flight file contributions,
cache members and materialized event bodies. Worker input and decoder scratch
are controlled by the per-file batch allowance and the source-record ceiling.
A slow callback stops further dispatch until it returns.

The Musenalm example accepts `--ndjson ordered` or `--ndjson replacements`:

```sh
cargo run -p objekte-search-core --example musenalm_cli -- \
  --fts Der --limit 5 --ndjson ordered
```

Each stdout line is JSON with `version: 1`, `type`, and `sequence`. Data types
are `ordered_rows`, `page_replacement`, and optional `progress`; terminal types
are `complete`, `cancelled`, and `error`. An ordered batch has `data.start` and
`data.rows`; a replacement has `data.rows`. `complete.final_page_sequence`
identifies the last page event, or zero for an empty page. The CLI writes
repository diagnostics to stderr in this mode. Existing collected output is
the default when `--ndjson` is absent.
