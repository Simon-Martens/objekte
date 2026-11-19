# Assignment 2 repair · Decode the offset of a resumed `pread64`

**Deliverable.** On `integration/core-refactoring-completion`, update `parse_events` and its existing `self_test` in `docs/ai/generated/core-refactoring-completion/assignment-2-source-preparation/trace_io.py`. The reviewed revision is `e6abf1a00eada9731dedb7466aaebb7b15771527`. Push the correction to the integration branch for the owner's merge.

## Reproduction

With fixture root `/fixture`, replay these two lines through `parse_events`.

```text
11 1.100010 pread64(3</fixture/data/00.json>,  <unfinished ...>
11 1.100011 <... pread64 resumed>"hi", 9, 7) = 2
```

The reviewed extractor raises `TraceError: unsupported split pread64 arguments`. The resumed handler already pairs the lines by thread ID and syscall name, but searches only `original["text"]` for the arguments. The completed call requested nine bytes at offset seven and returned two. It must emit exactly one read event for `/fixture/data/00.json` at offset **7**, with **2** bytes and the resumed-line timestamp. [Reviewed handler](https://github.com/Simon-Martens/objekte/blob/e6abf1a00eada9731dedb7466aaebb7b15771527/docs/ai/generated/core-refactoring-completion/assignment-2-source-preparation/trace_io.py#L78-L128)

## Correction

In the resumed `pread64` branch, join the saved text before the terminal `<unfinished ...>` marker to the text after the matched `<... pread64 resumed>` prefix. Parse the final offset argument at the closing parenthesis of the reassembled syscall. Use the successful syscall return value for the byte count. A positioned read leaves the tracked sequential file offset intact. If a successful in-scope call still lacks a valid offset, retain the explicit `TraceError`.

## Acceptance

Extend the existing optimized-Python self-test. Use the reproduction above and the following two splits of the same call. For each pair, check that exactly one event has the expected path, offset, byte count and timestamp.

| Last arguments printed before the unfinished marker | Text after the resumed prefix |
|---|---|
| `"hi", 9,` | `7) = 2` |
| `"hi", 9, 7` | `) = 2` |

The first row covers an offset printed alone on resumption. The second protects the previously working layout. Run the focused gate from the repository root and record the passing result with the integration commit SHA.

```sh
python3 -O docs/ai/generated/core-refactoring-completion/assignment-2-source-preparation/trace_io.py --self-test
```

## Implementation report

- Task identifier: `core-refactoring-completion-resumed-pread64-repair`
- Completion date: 2026-11-19
- Reviewed revision: `e6abf1a00eada9731dedb7466aaebb7b15771527`
- Integration branch and worktree: `integration/core-refactoring-completion` /
  `/home/simon/source/objekte-integration-core-refactoring-completion`

The resumed `pread64` branch now concatenates the original text with the
`<unfinished ...>` marker removed and the resumed text after the
`<... pread64 resumed>` prefix. The positioned offset is parsed from the final
integer argument of that completed call. Successful return values remain the
byte count, resumed timestamps remain event timestamps, and sequential offset
tracking is untouched.

The existing optimized-Python self-test now covers all three layouts:

1. reproduction: no offset before the marker and `"hi", 9, 7) = 2` on
   resumption;
2. count before the marker and offset alone on resumption;
3. the complete offset argument before the marker and only `) = 2` resumed.

Each case requires exactly one event with path `/fixture/data/00.json`, offset
7, 2 bytes and timestamp `1100011000`. The raw layouts are retained in
`trace-excerpt.log`.

The focused gate passed from the repository root:

```text
python3 -O docs/ai/generated/core-refactoring-completion/assignment-2-source-preparation/trace_io.py --self-test
trace extraction self-tests passed
```

This repair changes only verification tooling and companion evidence. It does
not change the core query implementation or invalidate the already recorded
Linux-only Rust validation and known formatter conflict. The integration
worktree remains for owner review.
