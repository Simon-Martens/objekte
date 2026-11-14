# Retry save prefills the active path

- Category: `feat`
- Originating task: `explicit-index-persistence`
- Branch: `integration/explicit-index-persistence`
- Location: `crates/desktop/src/ui/open_dialog.rs` (`show_save_index_dialog` calls `show_storage_dialog`)

After a save to a custom directory fails, the Index panel correctly offers **Retry save…** and keeps the existing repository session usable. The retry dialog derives its location and custom path from `active_open`, so it may prefill an earlier successful path instead of the failed destination. In the native Wayland smoke, a write-denied custom base failed as expected; Retry opened with the prior custom path. A subsequent explicitly selected Repository save succeeded.

Reproduction: open a repository, save to custom path A, then attempt to save to write-denied custom path B. Click **Retry save…** and inspect the prefilled path. Expected behavior for a convenient retry is path B, while still requiring the user to confirm the new explicit operation. The current behavior can make users choose the wrong destination if they assume Retry retained B. It does not cause an automatic write or data loss because the confirmation dialog remains open.

The UI stores the active opening draft but no distinct last attempted save draft. A possible approach is to keep the last attempted target solely for the save dialog and clear it when the session changes; this is a hypothesis for a follow-up, not a change authorized by filing this issue. The task's explicit-save correctness and retry availability are verified separately.
