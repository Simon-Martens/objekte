# Index-domain validation focused comparison

Fresh-process restore A used the unmodified `92cdb92b19` release binary; B
used the repair worktree with the all-logical validation fast path. Both read
the same checksummed snapshots (SHA-256 values in `manifest.json`), on the
same physical corpus and compiler, with affinity `0-31` and `powersave`
governor. Process blocks ran A-B-B-A, five restores per block and alphabet.
The decode phase includes snapshot read, decode **and validation**; it is not
pure deserialization. Filesystem reconciliation was measured separately.
Raw per-process data is in `restore-samples.jsonl`.

| Alphabet | Phase | A median ms | B median ms | Difference ms |
|---|---|---:|---:|---:|
| ASCII | Read/decode/validation | 392.901 | 302.016 | -90.885 |
| ASCII | Filesystem reconciliation | 206.354 | 209.541 | +3.187 |
| ASCII | Startup-to-ready | 602.425 | 515.892 | -86.533 |
| Unicode | Read/decode/validation | 449.398 | 313.944 | -135.454 |
| Unicode | Filesystem reconciliation | 205.827 | 207.198 | +1.371 |
| Unicode | Startup-to-ready | 659.706 | 523.093 | -136.613 |

Restore resident memory was approximately 237.0 MiB ASCII and 252.0 MiB
Unicode for both variants; peak memory was approximately 244.2 and 265.6
MiB. Snapshot sizes were 111,176,829 and 122,383,021 bytes. Every restore
reported zero build parses. The reduction occurs in read/decode/validation,
while reconciliation remains similar. The timing supports this repair's
validation-path attribution, though it does not isolate individual bitmap
traversals from other work inside that phase.

For fresh ASCII snapshot-disabled construction, `build-samples.jsonl` compares
the pre-validation binary at `d3f24b8` with the validation edit using three
separate processes each in A-B-B-A-A-B order. Median wall time was
11.758→12.053 s (+0.295 s), with 148,486 successful parses in every run and
236.3 MiB median peak RSS in both. This is within the observed build
variation; no build speedup is claimed. The final benchmark will cover both
alphabets and snapshot configurations with the added coarse discovery,
extraction/index, and finalization timers. Those timers do not read clocks in
the per-record loop. Healthy inventory and diagnostic heap estimates will be
reported separately from index bytes and RSS.
