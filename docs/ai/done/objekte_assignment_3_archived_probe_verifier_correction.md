# Assignment 3 archived relation probe verifier correction

The Rust source at `fd25d8678af5161cb92647d7dbddefa7f1aff391` passed the focused relation comparison. The remaining work is confined to the evidence verifier delivered at `9abedf6d2396550688f81a6f256d3b9de23f2bcc`. Use the existing `integration/core-refactoring-completion` worktree. The checked-in records supply all inputs for this correction. [Verifier](https://github.com/Simon-Martens/objekte/blob/9abedf6d2396550688f81a6f256d3b9de23f2bcc/docs/ai/generated/core-refactoring-completion/assignment-3-relation-verification-repair/verify_relation_repair.py#L242-L294)

## 1. Validate each archived ordered-result digest

In `validate_archived_probe`, parse each side’s `result-digests.json` with duplicate-key detection before constructing the case-to-digest map. Python’s ordinary JSON decoder keeps the last occurrence of a key. Require the expected case IDs exactly once per side. Require a string of 64 lowercase hexadecimal characters for every digest. Compare sides and count matches only after each side passes validation. Include the side, filename and case ID in malformed-digest and duplicate-key errors.

I independently reproduced two false passes for Q088. A second key with the committed value concealed a conflicting first value on the A2 side. Replacing the digest with JSON `null` on both sides also returned exit code 0 and reported one ordered-result match.

## 2. Validate the measured executable revision

In both archived scenarios, check each side’s `binary_build_revision` against its expected revision alongside the existing `code_revision` check. A2 must identify `9ae617b5df3f0a082aa923195a28019d5c36815d` and C must identify `acac94f486ee0f9d35eb5ba834d26cdc01eb1c5e`. Identify the side, field, expected value and observed value on failure. Changing only C’s binary revision to forty zeroes currently passes. All four committed manifests already contain the expected build revisions.

## Acceptance

Run the current control and five negative cases. Add three independent negative controls using temporary copies of the archived inputs.

| Mutation | Required result |
|---|---|
| Write two Q088 keys on A2, with the committed value last and a different valid digest first | Nonzero exit identifying the duplicate Q088 key |
| Set Q088 to JSON `null` on both sides | Nonzero exit identifying a malformed digest |
| Set only C’s `binary_build_revision` to a different 40-character revision | Nonzero exit identifying C’s executable revision mismatch |

With the original artifacts, the comparison must still reproduce the checked-in report, covering three traversal pairs and nine archived posting samples. Update the verifier self-test log and evidence index. The existing Rust tests and Q136 captures already cover the tested source.

## Implementation report

- Task identifier: `core-refactoring-completion-archived-probe-verifier-correction`
- Completion date: 2026-11-19
- Prior delivery: `9abedf6d2396550688f81a6f256d3b9de23f2bcc`
- Tested Rust source: `fd25d8678af5161cb92647d7dbddefa7f1aff391`
- Integration branch: `integration/core-refactoring-completion`
- Integration worktree: `/home/simon/source/objekte-integration-core-refactoring-completion`

This is an evidence-verifier-only correction. No Rust source changed.

### Digest and revision validation

The verifier now parses each archived `result-digests.json` through an ordered
key-pair hook, so a repeated case key is rejected instead of silently keeping
its last value. The top-level case set must equal the expected Q102/Q122 or
Q088 set, and every digest must be a string containing exactly 64 lowercase
hexadecimal characters. The two digest maps are compared only after both
sides pass those checks.

Duplicate-key and malformed-value diagnostics include the archived side,
filename and case ID. For Q088 they identify A2
`result-digests.json duplicate key Q088` and `Q088 malformed digest`
respectively.

Each side's run manifest now checks both `code_revision` and
`binary_build_revision`:

| Side | Expected source | Expected executable |
|---|---|---|
| A2 | `9ae617b5df3f0a082aa923195a28019d5c36815d` | `9ae617b5df3f0a082aa923195a28019d5c36815d` |
| C | `acac94f486ee0f9d35eb5ba834d26cdc01eb1c5e` | `acac94f486ee0f9d35eb5ba834d26cdc01eb1c5e` |

Mismatch diagnostics report the side, field, expected value and observed value.

### Self-test results

The current valid control passed. The original five negative controls still
failed for their intended reasons, and the verifier gains three independent
controls:

| Mutation | Observed result |
|---|---|
| A2 Q088 contains a valid conflicting key followed by the committed key | Rejected: `A2 result-digests.json duplicate key Q088` |
| Q088 is JSON `null` on both sides | Rejected: `Q088 malformed digest` |
| Only C's `binary_build_revision` is another 40-character revision | Rejected: `C run manifest binary_build_revision` |

The original artifacts still produce the checked-in comparison: three paired
relation traversal cases, nine archived posting samples, two Q102/Q122
ordering matches and one Q088 ordering match. The extended-schema artifacts
remain distinguished from a successful schema compatibility check.

The refreshed self-test log is
`docs/ai/generated/core-refactoring-completion/assignment-3-relation-verification-repair/verifier-self-test.log`.
The evidence index records all eight invalid controls. Existing Rust tests and
Q136 captures were not rerun because this correction does not alter their
source or inputs.
