# Paired final-matrix controls before the module move

Reference production core: `266d9a0fb071838be8f757671599872a7036a054`.
Candidate production core: `dd83a9a071` (before the contract-only module move).
The `manifest.json` files beside each result group record executable hashes, commands,
features, affinity and Rayon settings. `corpora-before.json` matches the frozen
baseline fingerprints for Musenalm, Lenz, Leipzig, Amazon and NYC. The synthetic
allocation fixture is recorded in `allocation-fixture.json`.

`paired-query-cpu-contended/` is retained as invalidated evidence: another CPU
intensive process ran during that measurement. The clean `paired-query/` and
`paired-query-tail/` runs replace it. The first sort runner had a summary-key
error after writing raw results; the corrected tail and additional relevant-sort
repeat are retained separately.

The original `reference-bin/core-allocation` is the first benchmark harness,
whose 2,000-file streaming fixture did not pin modification times. It therefore
returned different page IDs from run to run. `paired-core-allocation/` compares
the 22 cases with matching harness semantics. `paired-stream-fixture-v2/`
compares the remaining two cases using the same fixed-mtime benchmark source
against unchanged reference core. Both cases match complete signatures in all
five pairs. The reference v2 executable is identified by hash in that group's
manifest and is excluded from Git with the other generated executables.

The original timer-plus-counting-allocator index profile showed large gaps in
several build cases. `leipzig-harness-control/` rebuilds the reference with the
same feature-separated benchmark harness and unchanged production core; that
reference alone moves by about 234 ms in paired median for Leipzig FTS1.
`leipzig-modes/` and `leipzig-mode-d/` isolate timers and allocation counting.
`paired-index-profiles-normal/` measures all ten builds with the production
allocator and no stage timers. The large instrumented gap does not reproduce
there. Raw old-harness results remain in `paired-index-profiles/` and are not
presented as normal production latency.

The full query-cache runner, bigram encoding matrix, normal and instrumented
build profiles, restoration profiles and core allocation samples are saved here.
The completed-implementation report incorporates their case-level summaries and
states remaining limitations. Nothing under `candidate-bin/` or `snapshots/`
is committed.
