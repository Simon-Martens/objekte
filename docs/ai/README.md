# AI task workflow

`docs/ai/` contains specifications, working notes, issue reports, completion records, and user-requested generated documents for AI-assisted work on Objekte.

Follow this workflow for repository tasks. Explicit user instructions may authorize exceptions. A document's existence, an issue report, or an agent's own recommendation does not authorize additional work.

## Directory purposes

| Directory | Purpose | Agent action |
| --- | --- | --- |
| `tasks/` | User-supplied task specifications and supporting documents | Start only when explicitly instructed by the user |
| `memory/` | Temporary context for active tasks and subtasks | Read and update relevant notes; never commit |
| `issues/perf/` | Performance regressions or inefficient implementation | Record for user review |
| `issues/feat/` | Incomplete or inconsistent features | Record for user review |
| `issues/bug/` | Bugs or unexpected program behavior | Record for user review |
| `blocked/` | Tasks requiring substantial user decisions, review, or input | Record the decision needed and pause affected work |
| `done/` | Completed task specifications and implementation reports | Move completed task documents here, append reports, and commit and push them on the integration branch |
| `generated/` | User-requested reports, benchmark results, and other generated documents | Store requested documents here unless the user specifies another destination |

## 1. Task authorization and identity

- Start a task from `tasks/` only when the user explicitly instructs you to take it up. Authorization may name one task or an explicit set of tasks.
- Do not select tasks because they appear urgent, unfinished, or next in a directory listing.
- Never select work from `issues/`. The user may comment on an issue, move it to `tasks/`, and explicitly instruct you to implement it. Moving it alone is not authorization.
- Assign each task a stable, descriptive identifier, such as `query-cache-invalidation`. Use it consistently in branches, worktree names, notes, issue references, and reports.
- Each independently assigned task document gets its own integration branch and integration worktree. Supporting attachments referenced by that document belong to the same task.
- Read the complete assigned specification, relevant repository instructions, and existing task notes before implementation or resumption.

## 2. Branches, worktrees, and supervision

Every task uses:

- Integration branch: `integration/<task-id>`.
- Integration worktree: `../<repository-name>-integration-<task-id>`.
- Optional subtask branches: `task/<task-id>/<subtask-id>`.
- Optional subtask worktrees: `../<repository-name>-task-<task-id>-<subtask-id>`.

Resolve `../` relative to the original repository checkout. All task worktrees are siblings of that checkout, not children of it and not temporary directories elsewhere.

For example, an original checkout at `/home/simon/source/objekte` uses `/home/simon/source/objekte-integration-query-cache-invalidation` for its integration worktree.

Keep the original checkout on its existing main branch. Perform implementation, integration, validation, and commits in task worktrees. Local lifecycle updates to uncommitted task documents and temporary notes follow the document rules below and do not authorize code changes in the original checkout.

Use one supervising agent for each task. A single agent may act as both implementer and supervisor. Subagents are optional: use them when separable work benefits from delegation, not as a requirement for every task.

When delegating:

1. Give each subagent a defined subtask, ownership boundaries, acceptance criteria, and its branch and worktree.
2. Create subtask branches from the task's integration branch at a recorded commit.
3. Have subagents work only in their assigned worktrees. Only the supervisor writes to the integration worktree.
4. Require each subagent to report its pushed commit, validation results, outstanding issues, and relevant notes.
5. Have the supervisor review and merge subtask branches into the task's integration branch. Resolve conflicts there and validate the combined result.

Agents must never merge into, commit onto, or push updates to `main` or `master` unless explicitly instructed. Completion means the task is ready on its integration branch for the user's merge.

### Commit and push discipline

- Stage explicit task-owned paths or hunks. Review the staged diff before each commit. Do not use blanket staging that could include user documents or unrelated changes.
- Push immediately after every commit on the respective branch, including integration merge commits and report commits. Set the upstream on the first push.
- Confirm each push succeeded before making another commit on that branch or declaring the work complete.
- Use the repository's configured remote. Do not change remotes or credentials to resolve a push failure.
- If a push fails, retain the branch and worktree, diagnose the failure, and report the blocker. Do not discard commits or claim delivery. Independently authorized work may continue, but do not accumulate further commits on the affected branch.
- Do not force-push, rewrite published history, or reset user work unless explicitly authorized.

## 3. Task documents and Git state

Before modifying a task document, record its original path and whether it is clean and tracked, untracked, or locally modified/staged. Preserve its specification and user comments throughout its lifecycle.

| Initial document state | Handling before completion |
| --- | --- |
| Clean and committed | Maintain the authoritative document in the integration worktree. Commit lifecycle moves on the integration branch. The original checkout changes only when the user later merges. |
| Untracked | Keep the authoritative document in the original checkout. Leave it untracked while pending, in progress, or blocked. |
| Contains uncommitted or staged user changes | Preserve those changes and the user's index state. Maintain the authoritative working document in the original checkout; do not commit its uncommitted contents or lifecycle moves before completion unless explicitly instructed. |

Uncommitted documents do not appear automatically in other worktrees. Agents may read the authoritative document by absolute path or use a local reference copy. During execution, such copies remain uncommitted and are not independent task records. Use task-specific local excludes for untracked reference copies where needed; do not ignore all task documents or the completed documents in `done/`.

Only the supervisor moves authoritative task documents and appends the final report. Subagents provide their findings to the supervisor.

At completion, move the task document to `docs/ai/done/` in the integration worktree, append its implementation report, and commit and immediately push the completed document on the integration branch. This is required even when the task document was initially untracked or contained uncommitted user changes. The requirement to leave a task document uncommitted applies only before completion; this workflow explicitly authorizes committing the completed document and report.

When the authoritative source is in the original checkout, reconcile its latest contents and user comments into the completed document in the integration worktree. Include the old path's deletion in the integration commit if that path is tracked there. Do not stage, commit, or alter the index in the original checkout. After the completed document is committed and pushed, remove an obsolete untracked source only after verifying that all its contents are preserved and no newer edits exist. Retain tracked files and distinct staged or local changes in the original checkout for the user's merge; report any remaining local state that needs reconciliation. Report the final committed document path and commit.

Do not overwrite a destination file, drop user comments, or treat an ignore rule as protection for already tracked files. If concurrent user edits occur, reconcile them before moving or updating the document.

## 4. Temporary task memory

Use the original checkout's ignored directory as the shared location:

`docs/ai/memory/<task-id>/`

Create it when the task starts. Give all participating agents its absolute path. This allows agents in separate worktrees to use the same notes without committing them.

Each note must identify its task and, when applicable, its subtask. Use separate files for independently written notes to avoid concurrent overwrites. Record concise operational context:

- Task and subtask identifiers.
- Branch, worktree, and relevant commit.
- Decisions, constraints, and important findings.
- Work completed and next action.
- Validation results, unresolved questions, and related issue or blocker paths.

Read relevant notes when resuming after context loss. Notes support execution; they do not supersede the task specification or user instructions.

Nothing inside `memory/` may be committed, including placeholders. Never force-add memory files.

After a subtask has been merged and pushed into the integration branch, remove its obsolete notes after transferring any still-needed context to the supervisor's notes or the appropriate durable report.

Delete all remaining memory for a task when its complete implementation is integrated, validated, reported, and pushed. Do not wait for the user to merge into `main` or `master`. Retain memory for blocked or incomplete work, and never remove another task's notes.

## 5. Issues for user review

Record problems discovered during authorized work when they are outside scope or difficult enough to require separate work.

Use exactly these categories:

- `perf`: performance regressions, excessive allocation or copying, unnecessary computation, or other inefficient behavior.
- `feat`: incomplete features or inconsistent feature behavior.
- `bug`: incorrect results, failures, or unexpected program behavior.

Store each report under `issues/<category>/` with a descriptive filename. Include:

- Category, title, and originating task/subtask.
- Relevant branch or commit and code locations.
- Observed behavior and expected behavior.
- Reproduction steps or measured evidence, when available.
- Impact and why the problem was not resolved within the task.
- Possible approaches, explicitly distinguishing evidence from hypotheses.

New issue reports may be committed and pushed on the discovering task's branch. Preserve uncommitted user edits to existing reports.

Issue reports are strictly for user review. Creating one does not authorize implementing its proposed solution or delegating it as a new task.

If a discovered problem prevents the current task from meeting its acceptance criteria, do not mark that task complete merely because an issue was filed. Fix it within authorized scope or block the affected work and reference the issue.

## 6. Blocked tasks and user decisions

Move an affected task document to `blocked/` when progress requires a substantial user decision, user review, or missing input. Preserve its Git state according to section 3.

Examples include adding a dependency that was not explicitly authorized, restructuring substantial parts of the system, choosing between materially different behavior, or resolving missing requirements that affect correctness.

Do not block routine implementation choices within the agreed scope, or decisions the user has already made.

Append a blocker section containing:

- Task/subtask and current branch/worktree.
- Work completed and pushed commits.
- The exact missing decision, review, or input.
- Why it is required and which work depends on it.
- Concrete options, their consequences, and a recommendation when possible.
- Related issues and the conditions for resuming.

Prepare enough analysis or a reversible prototype to make the decision concrete without implementing the unapproved dependency or restructuring. Notify the user directly; placing a document in `blocked/` alone is insufficient.

Pause only the dependent work. Other authorized, independent subtasks may continue. For a blocked subtask, keep a separate task-tagged blocker record and reference it from the parent task; do not claim the parent complete.

When the user supplies the required decision and authorizes continuation, return the task to `tasks/`, preserve the decision in the document, and resume its existing integration branch and worktree. Do not create a second integration branch for the same task.

Blocked work retains its worktrees and necessary memory.

## 7. Validation and completion

A task is complete only when its agreed scope and acceptance criteria are satisfied, its changes are integrated, and required validation has been run.

During implementation, run checks appropriate to the changes. At the end of every large task, unless the user explicitly instructs otherwise:

1. Run the repository's workspace test suite against the combined integration result.
2. Run the existing performance benchmarks relevant to the changed code.
3. Record commands, environment, dataset, and results. Compare performance with the task's base revision under comparable conditions where applicable.

Treat changes spanning multiple components, changing shared interfaces, or substantially affecting execution or data handling as large tasks. Use repository-documented commands rather than inventing a new benchmark framework.

If no benchmark applies, state why. A documentation-only task does not require application performance measurements. If a required test or benchmark cannot run, record the reason and request the input or review needed; do not describe unverified work as passing.

Investigate material performance regressions before completion. If the agreed behavior would cause a substantial slowdown because the current data model does not fit it, explain the limitation and required design decision to the user.

The supervisor completes the task in this order:

1. Review and merge all completed subtasks into the integration branch, pushing after each resulting commit.
2. Validate the combined implementation and resolve remaining in-scope failures.
3. Move the authoritative task document from `tasks/` to `done/` in the integration worktree and append its implementation report, following section 3 for documents originating in the original checkout.
4. Always stage and commit the completed task document, including its implementation report and any tracked source-path deletion, on the integration branch. This includes previously uncommitted documents. Push immediately.
5. Confirm the integration branch and all task commits have been pushed.
6. Transfer necessary findings to the completion report or issue documents; delete the completed task's temporary memory.
7. Remove completed subordinate worktrees after the checks below. Keep the integration branch and integration worktree.
8. Report the integration branch, worktree, relevant commits, document path, validation results, and unresolved issues to the user.

Do not label a task complete while required decisions, in-scope failures, required validation, or a failed push remain unresolved, or while its completed task document and implementation report have not been committed and pushed to the integration branch.

### Implementation report

Append a section headed `Implementation report` to every completed task document. Include:

- Task identifier, completion date, base commit, and integration branch.
- Summary of implemented changes and how they meet the task.
- Subtasks, contributing branches, and implementation commits.
- Test commands and outcomes.
- Relevant benchmark commands, comparison results, and explained omissions.
- Approved deviations, remaining limitations, and issue references.
- Integration worktree path and cleanup status.

Record implementation commit identifiers before the report commit; do not attempt to include a commit's own identifier inside that same commit.

### Worktree cleanup

Remove subordinate worktrees only after their work is merged into the integration branch and pushed, and their validation results and important findings are retained.

Before removal, verify that each worktree belongs to this task and contains no unmerged work or unpreserved files. Preserve task documents, issue reports, user files, and any other necessary untracked files before removal.

Use normal Git worktree removal. Do not force removal, run broad cleanup commands, or delete unrelated directories to make cleanup succeed.

Keep the task's integration worktree and branch for the user. Branch deletion, including remote branch deletion, is not required and is not implicitly authorized by worktree cleanup.

If cleanup cannot be completed safely, retain the affected worktree and report its path and reason. Implementation completion and a cleanup exception must be reported separately.

## 8. Generated documents

Agents may store user-requested reports, benchmark results, analyses, and other generated documents in `docs/ai/generated/`, unless the user specifies another destination. Use descriptive filenames and identify the relevant task, revision, or benchmark configuration when applicable.

This directory holds requested deliverables, not temporary task memory or an additional task queue. Its contents do not authorize implementation work. It is not ignored; documents may be committed on the associated task branch, with an immediate push after every commit. The usual prohibition on commits to `main` or `master` still applies.

Keep each task's required implementation report appended to its task document in `done/`. It may link to a longer report or benchmark results in `generated/`, but a separate generated document does not replace the committed completion report.
