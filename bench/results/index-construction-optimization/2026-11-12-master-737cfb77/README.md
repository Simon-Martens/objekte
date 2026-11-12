# Index construction optimization evidence

Task `index-construction-optimization`, 12 November 2026. The baseline is master commit `737cfb7745ff15915804553ab583af260ac0f2e4` (B). The final measured integration code is `609474ee42e26c568cd43f89893482e0a0886bbe` on `integration/index-construction-optimization`. No master update is part of this archive.

Each `ascii-auto` or `unicode-auto` directory has `config.json`, `summary.json`, `runs.jsonl`, and one `sample.json` plus `run-manifest.json` per fresh-process warm-up or measured trial. `build_ns` is repository construction time in **nanoseconds**; `after_build_hwm` is the process high-water RSS in **bytes**, sampled immediately after construction and before optional queries or snapshot output. Trials were warm-cache, serial, and alternating order. The main comparison uses 16 resolved build workers. The `profile` directories are separate opt-in diagnostic runs and must not be mixed into unprofiled medians. Raw JSON retains the original ignored `target/index-construction-optimization/measurements/` path; replace that prefix with this archive to locate a copied trial.

| Directory | Control commit | Candidate commit | Purpose |
| --- | --- | --- | --- |
| `metadata/final-*`, `metadata/window64-vs32`, `metadata/profile-*`, `metadata/queries-*` | B or `1b0d4676` | `d5965bd1` | Bounded metadata preparation, worker scaling, window selection, semantics |
| `e1/` | B | `90057337` | Reusable source input buffer |
| `e2/` | B | `c6f60bff` | Worker-local bigram scratch |
| `e3/` | B | `e41b7f21` | Inline field selections |
| `e4/` | B | `66f42cc3` | Direct JSON-to-logical visitor |
| `a0/` | B | `82f7c1a8` | Admission component and scheduling diagnostics |
| `a1/` | `82f7c1a8` | `ce3bd850` | Dispatch replenishment between admissions |
| `final/` | B | `609474ee` | Selected combined code and snapshot/query checks |

The exact binary paths and SHA-256 hashes are in each comparison's `config.json`; full commands, compiler, corpus/schema fingerprints, worker count, governor, and dirty status are in each `run-manifest.json`. `compare.py` is the command driver. A representative invocation is:

```bash
python3 compare.py \
  --control-repo /home/simon/source/objekte-task-index-construction-optimization-baseline-measurement \
  --candidate-repo /home/simon/source/objekte-task-index-construction-optimization-final-measurement \
  --control-bin /home/simon/source/objekte/target/index-construction-optimization/bin/baseline-B \
  --candidate-bin /home/simon/source/objekte/target/index-construction-optimization/bin/integration-609474ee \
  --root /home/simon/source/objekte/examples/musenalm \
  --alphabet ascii --pairs 5 --out /home/simon/source/objekte/target/index-construction-optimization/measurements/final/ascii-auto
```

Use `--alphabet unicode` for the other final comparison. The measured configured corpus has 148,486 records, fingerprint `db869a76f26e1a3275a5162e5a207f47fb50fa8be02c4bdcfe3f1db50cfb747c`, and schema fingerprint `4115070eb30b0af98b109429d0883b493f857cdf671ec55b943d91b73c5ebfe3`. `host.json` records the CPU, kernel, compiler, and governor. The fixed query digests are Q001, Q108, Q116, Q120, and Q194.

`dhat/` contains Valgrind DHAT heap-allocation traces (with raw Valgrind text logs compressed as `valgrind.log.gz`) for B, E3, and E4, each using one worker and the **same** 776-record configured subset identified by `configured-subset-manifest.json`. These include process setup and post-build reporting as well as construction; they are allocation evidence, not elapsed-time claims. Summed DHAT program points: B **50,943,456 bytes / 284,002 blocks**, E3 **50,943,382 bytes / 283,875 blocks**, E4 **36,383,847 bytes / 258,973 blocks**. The fixture records and release binaries are omitted. Regenerate the fixture using `bench/generate_construction_profile_fixture.py --configured-subset --per-collection 128 --musenalm-root /home/simon/source/objekte/examples/musenalm --out <fixture>`.

`final/snapshot-baseline` and `final/snapshot-candidate` retain the benchmark samples/manifests, excluding each 129,139,458-byte snapshot file. `final/snapshot-hashes.txt` records their SHA-256 hashes. `verify_index_construction_snapshot.rs` is the temporary verifier source used with the combined code to open **both** baseline and candidate snapshots in `SnapshotMode::Required`; `final/restore-{baseline,candidate}.json` records zero build parses, restored=true, and identical five query digests. Snapshot files are omitted because they can be regenerated from the recorded commands. Byte hashes differ because current serialization does not promise byte-identical maps; size and restored behavior match.

`logs/` contains compressed workspace-test, Clippy, build, and focused experiment logs. Standard `cargo clippy --workspace --all-targets --locked` passed with existing warnings. An exploratory strict `-D warnings` invocation failed on 165 workspace lint findings, mostly pre-existing, and is archived separately. The integration workspace test log records 811 passed, 0 failed, 17 ignored across test groups.
