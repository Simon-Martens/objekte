# Gate A resolution, 2026-11-04

The matched FTS4 build comparison in [REPORT.md](REPORT.md) used the same
source paths and metadata, field overrides, production constructor and timed
boundary for reference and candidate. The benchmark-only patch separates
allocation counting and stage timers at compile time. Mode A uses the
production allocator with timers disabled. After the Clippy-only token fix,
its candidate build was 20.9 ms faster at 16 workers and 270.5 ms faster at
one worker in five paired runs. The earlier A series was also faster at both
worker counts; both patch and binary identities are retained separately.
No candidate slowdown was observed in those A samples.

The original 16-worker instrumented slowdown remains in the raw archive.
The new D series changed direction, and a reference-versus-reference D series
had paired differences from −223.9 to +292.0 ms. B1 and C1 each retained an
instrumentation-specific candidate slowdown above 100 ms. Allocation counting
added substantial 16-worker latency to both revisions. The precise cause of
the instrumented differences is unresolved; they are not used as a proxy for
production allocator latency.

Complete one-worker snapshot bytes and restoration matched across revisions
and a repeated reference build. Sixteen-worker snapshot bytes were
nondeterministic even between reference builds, although sizes, document
counts and index component sizes matched. The previous failed byte-hash
diagnostic is retained.

**Decision:** Gate A is supported for the measured production-like FTS4
construction workload, with the instrumentation behavior explicitly bounded
as a separate diagnostic finding. No production implementation change was
needed to reach this result. Gate B registered-representation indexing and
Gate C default/registered query performance remain open. This is not a claim
that the full library extension migration is complete or regression-free.
