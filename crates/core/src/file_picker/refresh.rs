//! Apply inventory changes and individual file edits to live indexes.

use super::projection::{Projection, refreshed_values};
use super::typed::{PathValues, TypedKey};
use super::*;
use crate::repository::SourceChanges;


struct StagedProjection {
    malformed: bool,
    present_fields: Vec<usize>,
    array_fields: Vec<String>,
    view_keys: Vec<crate::query::Scalar>,
    sort_keys: Vec<Vec<crate::sorting::SortValue>>,
    bigrams: Option<Vec<u16>>,
    typed_values: Vec<(usize, TypedKey)>,
    path_values: Vec<PathValues>,
}


impl StagedProjection {
    fn from_projection(
        (projection, path_values): (Option<Projection>, Vec<PathValues>),
        view: &ViewIndex,
        sort: &SortIndex,
        encoding: BigramEncoding,
        overlay: bool,
    ) -> Self {
        match projection {
            Some(projection) => Self {
                malformed: false,
                present_fields: projection.present_fields,
                array_fields: projection.array_fields,
                view_keys: projection.view_keys,
                sort_keys: projection.sort_keys,
                bigrams: overlay.then(|| sorted_bigrams(&projection.bytes, encoding)),
                typed_values: projection.typed_values,
                path_values,
            },
            None => Self {
                malformed: true,
                present_fields: Vec::new(),
                array_fields: Vec::new(),
                view_keys: view.missing_keys(),
                sort_keys: sort.missing_keys(),
                bigrams: None,
                typed_values: Vec::new(),
                path_values: Vec::new(),
            },
        }
    }
}


struct PendingRefresh {
    id: String,
    stamp: Stamp,
    slot: Option<usize>,
    projection: StagedProjection,
}


impl FilePicker {
    fn stage_refresh_projection(
        &self,
        source: &Source,
        id: &str,
        stamp: &Stamp,
        overlay: bool,
    ) -> Result<StagedProjection> {
        let projection = self.refreshed(source, id, stamp)?;
        Ok(StagedProjection::from_projection(
            projection,
            &self.view,
            &self.sort,
            self.encoding,
            overlay,
        ))
    }

    /// Read one file again for every live index, including typed postings.
    fn refreshed(
        &self,
        source: &Source,
        id: &str,
        stamp: &Stamp,
    ) -> Result<(Option<Projection>, Vec<PathValues>)> {
        refreshed_values(
            source,
            id,
            stamp,
            self.typed.fields(),
            self.presence.fields(),
            &self.view,
            &self.sort,
            &self.typed.dynamic_paths(),
        )
    }

    fn apply_staged_projection(
        &mut self,
        slot: usize,
        staged: StagedProjection,
        overlay: bool,
    ) -> (
        Vec<crate::query::Scalar>,
        Vec<Vec<crate::sorting::SortValue>>,
    ) {
        if staged.malformed {
            self.malformed.insert(slot);
        } else {
            self.malformed.remove(&slot);
        }
        self.presence.update(slot, &staged.present_fields);
        self.array_fields.extend(staged.array_fields);
        self.typed.patch(
            slot,
            self.files.len(),
            &staged.typed_values,
            staged.path_values,
        );
        if overlay {
            if let Some(bigrams) = staged.bigrams {
                self.changed_bigrams.insert(slot, bigrams);
            } else {
                self.changed_bigrams.remove(&slot);
            }
        }
        (staged.view_keys, staged.sort_keys)
    }

    /// Consume a local projection after all borrowed index computations finish.
    /// New slots lie beyond the immutable index, so only existing slots need
    /// changed-bigram overlays.
    fn apply_refresh_projection(
        &mut self,
        slot: usize,
        projection: (Option<Projection>, Vec<PathValues>),
        overlay: bool,
    ) -> (
        Vec<crate::query::Scalar>,
        Vec<Vec<crate::sorting::SortValue>>,
    ) {
        let staged = StagedProjection::from_projection(
            projection,
            &self.view,
            &self.sort,
            self.encoding,
            overlay,
        );
        self.apply_staged_projection(slot, staged, overlay)
    }

    pub(super) fn update_restored_slot(&mut self, source: &Source, slot: usize) -> Result<()> {
        let id = self.id(slot).expect("restored slot").to_owned();
        let stamp = self.files[slot]
            .as_ref()
            .expect("restored file")
            .stamp
            .clone();
        let projection = self.refreshed(source, &id, &stamp)?;
        let (keys, sort_keys) = self.apply_refresh_projection(slot, projection, true);
        self.key_summary.saturate();
        let entry = self.files[slot].as_mut().expect("restored file");
        entry.revision += 1;
        entry.changed_at_nanos = now_nanos();
        self.view.update(slot, keys, &self.files, &self.id_bytes);
        self.sort
            .update(slot, sort_keys, &self.files, &self.id_bytes);
        self.modified_order.touch(slot, &self.files, &self.id_bytes);
        Ok(())
    }

    /// Refreshes inventory from the authoritative directory. A few edits only
    /// enter the overlay; they never force a full bigram rebuild.
    pub fn refresh(&mut self) -> Result<usize> {
        Ok(self.refresh_changes()?.len())
    }

    /// Like `refresh`, but reports the created, changed, and deleted IDs.
    pub(crate) fn refresh_changes(&mut self) -> Result<SourceChanges> {
        let source = Source::open_scoped(&self.root, self.recursive)?;
        let inventory = source.inventory()?;
        let changes = self.refresh_from_inventory(&source, inventory)?;
        source.finish()?;
        self.verified = true;
        self.compact_large_overlay()?;
        Ok(changes)
    }

    pub(crate) fn reconcile_inventory(
        &mut self,
        inventory: BTreeMap<String, Stamp>,
    ) -> Result<usize> {
        let source = Source::open_scoped(&self.root, self.recursive)?;
        let changes = self.refresh_from_inventory(&source, inventory)?;
        source.finish()?;
        self.verified = true;
        self.compact_large_overlay()?;
        Ok(changes.len())
    }

    fn refresh_from_inventory(
        &mut self,
        source: &Source,
        inventory: BTreeMap<String, Stamp>,
    ) -> Result<SourceChanges> {
        let old_slots = self.files.len();
        let mut seen = vec![false; old_slots];
        let mut pending = Vec::new();
        let mut added_bytes = 0usize;
        // All fallible source reads precede publication. Staging retains keys
        // and bigrams, but drops parsed documents and source bytes immediately.
        for (id, stamp) in inventory {
            let slot = self
                .position(&id)
                .ok()
                .map(|position| self.sorted[position]);
            if let Some(slot) = slot {
                seen[slot] = true;
                if self.files[slot].as_ref().expect("live sorted slot").stamp == stamp {
                    continue;
                }
            } else {
                added_bytes = added_bytes
                    .checked_add(id.len())
                    .ok_or_else(|| Error::Cache("file picker ID arena exceeds 4 GiB".into()))?;
                u32::try_from(id.len())
                    .map_err(|_| Error::Cache("file picker ID exceeds 4 GiB".into()))?;
            }
            let projection = self.stage_refresh_projection(source, &id, &stamp, slot.is_some())?;
            pending.push(PendingRefresh {
                id,
                stamp,
                slot,
                projection,
            });
        }
        let deleted = (0..old_slots)
            .filter(|&slot| !seen[slot] && self.files[slot].is_some())
            .collect::<Vec<_>>();
        let changed = pending.len() + deleted.len();
        if changed == 0 {
            return Ok(SourceChanges::default());
        }
        let mut changes = SourceChanges::default();
        let final_arena = self
            .id_bytes
            .len()
            .checked_add(added_bytes)
            .ok_or_else(|| Error::Cache("file picker ID arena exceeds 4 GiB".into()))?;
        u32::try_from(final_arena)
            .map_err(|_| Error::Cache("file picker ID arena exceeds 4 GiB".into()))?;
        source.finish()?;
        self.verified = false;

        // A handful of edits are cheaper with the existing local insertion.
        // For a burst, shift each maintained order once, not once per file.
        const BATCH_THRESHOLD: usize = 32;
        let batch = changed >= BATCH_THRESHOLD;
        let additions = pending.iter().filter(|entry| entry.slot.is_none()).count();
        let mut affected = if batch {
            vec![false; old_slots + additions]
        } else {
            Vec::new()
        };
        let mut live_changed = Vec::with_capacity(pending.len());
        let mut added = Vec::with_capacity(additions);
        for entry in pending {
            let existing = entry.slot.is_some();
            let slot = if let Some(slot) = entry.slot {
                let file = self.files[slot].as_mut().expect("live sorted slot");
                file.stamp = entry.stamp;
                file.revision += 1;
                file.changed_at_nanos = now_nanos();
                self.changed.insert(slot);
                slot
            } else if batch {
                let revision = self
                    .tombstones
                    .get(&entry.id)
                    .map_or(1, |(revision, _)| revision + 1);
                let slot = self.append_slot(&entry.id, entry.stamp, revision)?;
                self.tombstones.remove(&entry.id);
                self.changed.insert(slot);
                added.push(slot);
                slot
            } else {
                self.insert(&entry.id, entry.stamp)?
            };
            let (view_keys, sort_keys) =
                self.apply_staged_projection(slot, entry.projection, existing);
            if batch {
                affected[slot] = true;
                live_changed.push(slot);
                self.view.set_keys(slot, view_keys);
                self.sort.set_keys(slot, sort_keys);
            } else {
                self.view
                    .update(slot, view_keys, &self.files, &self.id_bytes);
                self.sort
                    .update(slot, sort_keys, &self.files, &self.id_bytes);
            }
            if existing || batch {
                self.modified_order.touch(slot, &self.files, &self.id_bytes);
            }
            if existing {
                changes.changed.push(entry.id);
            } else {
                changes.created.push(entry.id);
            }
        }
        for slot in deleted {
            let entry = self.files[slot].take().expect("deleted live slot");
            let start = entry.id_start as usize;
            let end = start + entry.id_len as usize;
            let id = std::str::from_utf8(&self.id_bytes[start..end])
                .expect("validated ID")
                .to_owned();
            self.tombstones
                .insert(id.clone(), (entry.revision + 1, now_nanos()));
            changes.deleted.push(id);
            self.changed.remove(&slot);
            self.changed_bigrams.remove(&slot);
            self.malformed.remove(&slot);
            self.presence.remove(slot);
            self.typed.patch(slot, self.files.len(), &[], Vec::new());
            if batch {
                affected[slot] = true;
            } else {
                self.view.remove(slot);
                self.sort.remove(slot);
            }
            self.modified_order.touch(slot, &self.files, &self.id_bytes);
        }
        if batch {
            let old_order = std::mem::take(&mut self.sorted);
            let mut old = old_order.into_iter().filter(|&slot| seen[slot]).peekable();
            let mut new = added.iter().copied().peekable();
            let mut merged = Vec::with_capacity(
                self.files.len() - old_slots + seen.iter().filter(|&&live| live).count(),
            );
            while let (Some(&left), Some(&right)) = (old.peek(), new.peek()) {
                if self.id(left).expect("live ID") <= self.id(right).expect("new ID") {
                    merged.push(old.next().unwrap());
                } else {
                    merged.push(new.next().unwrap());
                }
            }
            merged.extend(old);
            merged.extend(new);
            self.sorted = merged;
            self.view
                .merge_changed(&affected, &live_changed, &self.files, &self.id_bytes);
            self.sort
                .merge_changed(&affected, &live_changed, &self.files, &self.id_bytes);
        } else {
            self.sorted.retain(|&slot| slot >= old_slots || seen[slot]);
        }
        self.modified_order
            .maybe_compact(&self.files, &self.id_bytes);
        self.key_summary.saturate();
        changes.deleted.sort();
        Ok(changes)
    }

    /// Updates a single path after an application edit, without
    /// enumerating the directory.
    pub fn refresh_id(&mut self, id: &str) -> Result<()> {
        self.refresh_ids(&[id.to_owned()]).map(|_| ())
    }

    /// Reconcile listed IDs with their current source files: a present file
    /// is created or changed, an absent one deleted. Unchanged IDs are skipped.
    pub(crate) fn refresh_ids(&mut self, ids: &[String]) -> Result<SourceChanges> {
        for id in ids {
            crate::repository::validate_id(id)?;
        }
        let source = Source::open_scoped(&self.root, self.recursive)?;
        let mut changes = SourceChanges::default();
        for id in ids {
            self.refresh_one(&source, id, &mut changes)?;
        }
        source.finish()?;
        if !changes.is_empty() {
            self.key_summary.saturate();
        }
        self.modified_order
            .maybe_compact(&self.files, &self.id_bytes);
        self.compact_large_overlay()?;
        Ok(changes)
    }

    fn refresh_one(
        &mut self,
        source: &Source,
        id: &str,
        changes: &mut SourceChanges,
    ) -> Result<()> {
        let file = match source.file(id) {
            Ok(file) => Some(file),
            Err(Error::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        match file {
            Some(file) => {
                let path = self.root.join(format!("{id}.json"));
                let stamp =
                    Stamp::from_metadata(&file.metadata().map_err(|e| io(&path, e))?, &path)?;
                match self.position(id) {
                    Ok(position) => {
                        let slot = self.sorted[position];
                        if self.files[slot].as_ref().expect("live sorted slot").stamp != stamp {
                            let projection = self.refreshed(source, id, &stamp)?;
                            let entry = self.files[slot].as_mut().expect("live sorted slot");
                            entry.stamp = stamp;
                            entry.revision += 1;
                            entry.changed_at_nanos = now_nanos();
                            self.changed.insert(slot);
                            let (keys, sort_keys) =
                                self.apply_refresh_projection(slot, projection, true);
                            self.view.update(slot, keys, &self.files, &self.id_bytes);
                            self.sort
                                .update(slot, sort_keys, &self.files, &self.id_bytes);
                            self.modified_order.touch(slot, &self.files, &self.id_bytes);
                            changes.changed.push(id.to_owned());
                        }
                    }
                    Err(_) => {
                        let projection = self.refreshed(source, id, &stamp)?;
                        let slot = self.insert(id, stamp)?;
                        let (keys, sort_keys) =
                            self.apply_refresh_projection(slot, projection, false);
                        self.view.update(slot, keys, &self.files, &self.id_bytes);
                        self.sort
                            .update(slot, sort_keys, &self.files, &self.id_bytes);
                        changes.created.push(id.to_owned());
                    }
                }
            }
            None => {
                if let Ok(position) = self.position(id) {
                    let slot = self.sorted.remove(position);
                    if let Some(entry) = self.files[slot].take() {
                        self.tombstones
                            .insert(id.to_owned(), (entry.revision + 1, now_nanos()));
                    }
                    self.changed.remove(&slot);
                    self.changed_bigrams.remove(&slot);
                    self.malformed.remove(&slot);
                    self.presence.remove(slot);
                    self.typed.patch(slot, self.files.len(), &[], Vec::new());
                    self.view.remove(slot);
                    self.sort.remove(slot);
                    self.modified_order.touch(slot, &self.files, &self.id_bytes);
                    changes.deleted.push(id.to_owned());
                }
            }
        }
        Ok(())
    }
}
