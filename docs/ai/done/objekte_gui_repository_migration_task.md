# Objekte GUI repository migration task

Date: 8 November 2026

The Fedora GPUI environment setup and headless Wayland follow-up have already been implemented. Treat the working implementation as the baseline. This is a new task to migrate application code and development tooling from the separate `gpui-development-environment` repository into the existing Objekte repository.

Do not repeat either completed setup task. Preserve the working Sway session configuration, installed dependencies, compatible GPUI versions, graphics drivers, and useful build caches. The objective is to correct repository ownership while retaining the verified behavior and the implementation knowledge acquired while establishing it.

## 1. Required result

All native desktop application source, validation examples, GUI control scripts, and configuration templates belong to Objekte. They must build and operate from an Objekte checkout without a source or runtime dependency on the old repository.

Reuse an existing desktop package if the local Objekte checkout already has one. Otherwise use this layout:

| Path relative to Objekte | Purpose |
| --- | --- |
| `crates/desktop/` | Actual desktop application package, named `objekte-desktop` if newly created |
| `crates/desktop/examples/environment_validation.rs` | Existing environment verification program, migrated as an example |
| `scripts/gui/` | Existing dependency, session, screenshot, input, and viewing scripts |
| `scripts/gui/config/` | Sway configuration and task-owned service templates |
| `docs/development/gui_environment.md` | Verified operating procedure, architecture decisions, and troubleshooting knowledge |
| `docs/ai/generated/gui_repository_migration_report.md` | Migration inventory, validation results, and remaining limitations |
| `target/gui-validation/` | Ignored generated screenshots, fixtures, and logs |

Keep the production application and environment verification example distinct. If the separate repository contains only a verification program, migrate that program without inventing or implementing the full product interface. Preserve and integrate any actual application work that already exists.

## 2. Worktree and execution rules

Read Objekte's current `AGENTS.md`, `docs/ai/` instructions, manifests, and Git status. Keep the original checkout on master with its files and index unchanged.

Create `integration/gui-repository-migration` in the Objekte repository, with a worktree at `/home/simon/source/objekte-integration-gui-repository-migration`, or reuse the appropriate existing Objekte worktree for this task. Never create worktrees under `/tmp`. A branch in the separate environment repository is not an Objekte integration branch.

Leave one integration branch for the user to merge. Do not merge into master or push. Apply the existing repository rules for task documents, completion reports, and commits.

One integrator owns service updates and live GUI validation. If parallel agents are useful, separate application/example migration from script/documentation migration in their own Objekte worktrees. Integrate them before exercising the shared environment; do not operate the display or modify host services concurrently.

## 3. Inventory and preserve the working baseline

Inspect the actual source repository, normally `/home/simon/source/gpui-development-environment`, without changing it. Record its revision, tracked and untracked work, executable targets, dependency versions, configuration, and task-owned services. Inspect ignored files only as needed to identify runtime state, caches, and useful uncommitted source; do not copy them indiscriminately.

Record the currently working launch and validation commands, active service paths, socket discovery method, and configured display geometry. The established target is 1920 × 1080 at scale 1 and nominal 60 Hz. If the implementation differs, record that as a pre-existing discrepancy rather than claiming migration changed it.

Capture a minimal baseline result if the session is available. Reuse its prior report and evidence for expensive checks. Do not reinstall packages, upgrade dependencies, reset either repository, clean Cargo caches, or replace working graphics configuration.

### Preserve implementation knowledge

The completed setup is a tested reference implementation. Transfer its practical knowledge into Objekte along with its source. Inspect existing README files, setup and validation reports, scripts, service definitions, relevant commit messages, and available diagnostic evidence. Preserve useful findings before removing obsolete instructions from the migrated copies. Do not depend on the original conversation or the old repository for future operation.

Consolidate the following in `docs/development/gui_environment.md`, using the implementation that actually worked:

| Knowledge | Required content |
| --- | --- |
| Architecture and rationale | How SSH commands reach the compositor and application; why the selected session, rendering, input, and screenshot arrangements were chosen |
| Prerequisites | Verified package/toolchain compatibility, graphics requirements, and necessary user permissions; identify which settings are specific to the tested host |
| Operating procedure | Exact start, discovery, build, launch, focus, input, screenshot, validation, reconnect, and cleanup commands, with expected success indicators |
| Diagnosed failures | Observed symptom or error, confirmed cause where known, working correction, and relevant version or host conditions |
| Performance findings | Recorded startup, build, interaction, screenshot, and transfer measurements with their conditions, plus changes that materially improved them |
| Known limits | Capabilities actually verified, remaining problems, and untested alternatives |

Explain the reasons for essential environment variables and service settings, rather than leaving unexplained configuration to copy. Distinguish measured results and confirmed diagnoses from hypotheses. A failed attempt on one host or version must not become an unsupported claim that the approach can never work. Reuse existing evidence; do not repeat failed experiments or the full benchmark campaign merely to document them.

Keep the general procedure in the maintained development guide. Preserve exact tested versions, host-specific observations, evidence references, and migration provenance in the migration report or existing environment report. Link these documents and add a short discovery link in the existing `docs/ai/README.md` or equivalent agent instruction entry point. Keep instructions in one maintained location instead of duplicating them across agent files. Exclude credentials and unrelated private data from transferred diagnostics.

Validate the written procedure during the fresh-SSH acceptance check below. The guide must let a future developer or agent operate the established environment without rediscovering its setup. Another machine may require adaptation; document the tested assumptions instead of claiming universal compatibility.

## 4. Migrate source and integrate the workspace

1. Copy the required source, assets, configuration templates, and scripts into the selected Objekte paths. Review destination conflicts and preserve existing Objekte code. Do not copy `.git`, credentials, generated build directories, or the source repository's root workspace manifest and independent lockfile.
2. Register the desktop package in Objekte's existing Cargo workspace. Reuse the verified GPUI/GPUI Kit dependency pairing. Integrate required dependencies into the existing root lockfile with minimal unrelated change; review the resolution and then use locked builds. Do not create a nested Cargo workspace.
3. Where application code needs the Objekte library, use the existing core package through a local workspace dependency. Do not duplicate the core library or point at another checkout.
4. Keep GUI dependencies out of core, CLI, and TUI packages. Preserve the existing default build scope, using explicit GUI package/example commands and appropriate `default-members` if adding the GUI would otherwise make ordinary builds unnecessarily expensive. Workspace-wide builds intentionally include the new member.
5. Make scripts resolve the repository root and the selected package/example from their location or explicit configuration. Move generated output into an ignored directory. Remove active script and configuration references to the old repository; migration documentation may retain them.

Suggested commit: `Migrate GUI source and tools into the Objekte workspace`.

## 5. Update service paths without rebuilding the environment

Preserve the working compositor configuration and service behavior. Change only paths and launch arguments needed to run Objekte-owned files. Record the previous task-owned service configuration so it can be restored if validation fails.

Use the existing independent Sway session where possible. Restart only task-owned services when required to activate a path change. Do not restart the user's physical desktop, reinstall Sway, change drivers, recreate the display architecture, or broaden this task into environment troubleshooting.

Installed user-service files may contain resolved paths to the integration worktree. Provide a repeatable installation/update command that derives those paths from the selected Objekte checkout. Document running it again after the user merges and switches to the main checkout, before the integration worktree is removed. Do not leave an undocumented permanent dependency on a temporary worktree.

Suggested commit: `Update GUI service and command paths for Objekte`.

## 6. Validate the migrated workflow

Run from the Objekte integration worktree and its selected executable targets:

1. Build the migrated validation example and any existing application with the integrated lockfile. Do not require a product binary that has not yet been implemented.
2. Start or reuse the working headless session through the migrated scripts. Verify display geometry and the recorded hardware rendering path.
3. Launch the migrated example, inspect a screenshot, enter `Objekte ÄÖÜ äöü ß 123`, click the existing controls, scroll, and save. Verify the resulting JSON and final image.
4. Follow the maintained operating guide from a fresh SSH connection to repeat discovery and application launch. Confirm the documented commands work and that the launched executable and task-owned service paths belong to the Objekte checkout and its build artifacts.
5. Confirm that active scripts, local dependencies, service commands, and configuration no longer reference files in the old repository. Keep the old repository in place; do not rename or delete it merely to prove independence.
6. Perform the repository checks appropriate to the changed packages and scripts. Run one brief timing comparison against the existing baseline if available; investigate a material regression without repeating the complete environment benchmark campaign.

If a pre-existing environment problem prevents a check, distinguish it from a migration regression and report the precise blocker. Do not claim a successful migration based only on copied files or compilation.

## 7. Completion

Commit the final documentation and migration report on the Objekte integration branch. Record the source revision, migrated files, preserved uncommitted work, workspace changes, service path changes, validation outcomes, and the post-merge service-update command. Identify where the acquired implementation knowledge, diagnosed failures, and performance evidence have been preserved, and link the operating guide from the agent instruction entry point.

Leave `/home/simon/source/gpui-development-environment` intact as a recovery source. Its removal is a separate cleanup decision after the migrated workflow has been verified. Do not continue new development there.

Completion means the existing GUI workflow operates from Objekte, its acquired implementation knowledge is documented and discoverable by future agents, the original master checkout remains untouched, and the user receives one Objekte integration branch to merge.

## Implementation report

Completed 8 November 2026 as task `gui-repository-migration` on `integration/gui-repository-migration`, based on Objekte commit `a5cf0298d99bf15701cf632e6d29a6ed59f9f5b5`. The integration worktree is `/home/simon/source/objekte-integration-gui-repository-migration`.

The working GPUI validation example, Sway configuration, session/control scripts, and dependency checks were migrated into Objekte. The example builds and passes a native 1920 × 1080, scale 1, 60 Hz validation cycle, including a fresh SSH run with Unicode text, pointer input, scrolling, screenshots, and GUI Save. The new package is an example-only desktop package because the source contained no product desktop UI. Task-owned services now point to Objekte files; the original environment repository and original Objekte master checkout remain intact.

Validation passed: locked example build, GPUI Kit example test, full locked workspace tests, formatting, shell and Python syntax checks, visual screenshot review, exact JSON fixture, and a brief screenshot timing comparison. Two pre-existing CLI/TUI assertions were corrected for JSON field order when GPUI's `serde_json/preserve_order` feature is unified in a workspace build. No new core benchmark was run because core implementation did not change. The [migration report](../generated/gui_repository_migration_report.md) contains the inventory, versions, service paths, evidence, timings, and limits; the maintained [operating guide](../../development/gui_environment.md) contains the daily commands and post-merge service update.

The task explicitly prohibited pushing, so this completion is a local integration-branch commit. The untracked source task document remains in the original checkout to preserve its state until the user merges.
