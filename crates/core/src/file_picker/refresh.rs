//! Apply inventory changes and individual file edits to live indexes.

use super::*;


impl FilePicker {
    pub(super) fn update_restored_slot(&mut self, source: &Source, slot: usize) -> Result<()> {
        let id = self.id(slot).expect("restored slot").to_owned();
        let stamp = self.files[slot]
            .as_ref()
            .expect("restored file")
            .stamp
            .clone();
        let projection = projected_values(
            source,
            &id,
            &stamp,
            &[],
            self.presence.fields(),
            &self.view,
            &self.sort,
            true,
        )?;
        let keys = projection
            .as_ref()
            .map_or_else(|| self.view.missing_keys(), |p| p.view_keys.clone());
        let sort_keys = projection
            .as_ref()
            .map_or_else(|| self.sort.missing_keys(), |p| p.sort_keys.clone());
        if projection.is_none() {
            self.malformed.insert(slot);
            self.changed_bigrams.remove(&slot);
        } else {
            self.malformed.remove(&slot);
        }
        self.presence.update(
            slot,
            projection
                .as_ref()
                .map_or(&[], |p| p.present_fields.as_slice()),
        );
        if let Some(projection) = projection.as_ref() {
            self.array_fields
                .extend(projection.array_fields.iter().cloned());
            self.changed_bigrams
                .insert(slot, sorted_bigrams(&projection.bytes, self.encoding));
        }
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
        let source = Source::open_scoped(&self.root, self.recursive)?;
        let inventory = source.inventory()?;
        let changed = self.refresh_from_inventory(&source, inventory)?;
        source.finish()?;
        self.verified = true;
        Ok(changed)
    }

    pub(crate) fn reconcile_inventory(
        &mut self,
        inventory: BTreeMap<String, Stamp>,
    ) -> Result<usize> {
        let source = Source::open_scoped(&self.root, self.recursive)?;
        let changed = self.refresh_from_inventory(&source, inventory)?;
        source.finish()?;
        self.verified = true;
        Ok(changed)
    }

    fn refresh_from_inventory(
        &mut self,
        source: &Source,
        inventory: BTreeMap<String, Stamp>,
    ) -> Result<usize> {
        let mut seen = vec![false; self.files.len()];
        let mut changed = 0;
        for (id, stamp) in inventory {
            match self.position(&id) {
                Ok(position) => {
                    let slot = self.sorted[position];
                    seen[slot] = true;
                    if self.files[slot].as_ref().expect("live sorted slot").stamp != stamp {
                        let projection = projected_values(
                            source,
                            &id,
                            &stamp,
                            &[],
                            self.presence.fields(),
                            &self.view,
                            &self.sort,
                            true,
                        )?;
                        let keys = projection
                            .as_ref()
                            .map_or_else(|| self.view.missing_keys(), |p| p.view_keys.clone());
                        let sort_keys = projection
                            .as_ref()
                            .map_or_else(|| self.sort.missing_keys(), |p| p.sort_keys.clone());
                        if projection.is_none() {
                            self.malformed.insert(slot);
                        } else {
                            self.malformed.remove(&slot);
                        }
                        self.presence.update(
                            slot,
                            projection
                                .as_ref()
                                .map_or(&[], |p| p.present_fields.as_slice()),
                        );
                        if let Some(projection) = projection.as_ref() {
                            self.array_fields
                                .extend(projection.array_fields.iter().cloned());
                        }
                        let entry = self.files[slot].as_mut().expect("live sorted slot");
                        entry.stamp = stamp;
                        entry.revision += 1;
                        entry.changed_at_nanos = now_nanos();
                        self.changed.insert(slot);
                        if let Some(projection) = projection.as_ref() {
                            self.changed_bigrams
                                .insert(slot, sorted_bigrams(&projection.bytes, self.encoding));
                        } else {
                            self.changed_bigrams.remove(&slot);
                        }
                        self.view.update(slot, keys, &self.files, &self.id_bytes);
                        self.sort
                            .update(slot, sort_keys, &self.files, &self.id_bytes);
                        self.modified_order.touch(slot, &self.files, &self.id_bytes);
                        changed += 1;
                    }
                }
                Err(_) => {
                    let projection = projected_values(
                        source,
                        &id,
                        &stamp,
                        &[],
                        self.presence.fields(),
                        &self.view,
                        &self.sort,
                        true,
                    )?;
                    let keys = projection
                        .as_ref()
                        .map_or_else(|| self.view.missing_keys(), |p| p.view_keys.clone());
                    let sort_keys = projection
                        .as_ref()
                        .map_or_else(|| self.sort.missing_keys(), |p| p.sort_keys.clone());
                    if let Some(projection) = projection.as_ref() {
                        self.array_fields
                            .extend(projection.array_fields.iter().cloned());
                    }
                    let slot = self.insert(&id, stamp)?;
                    if projection.is_none() {
                        self.malformed.insert(slot);
                    }
                    self.presence.update(
                        slot,
                        projection
                            .as_ref()
                            .map_or(&[], |p| p.present_fields.as_slice()),
                    );
                    self.view.update(slot, keys, &self.files, &self.id_bytes);
                    self.sort
                        .update(slot, sort_keys, &self.files, &self.id_bytes);
                    seen.push(true);
                    debug_assert_eq!(slot + 1, seen.len());
                    changed += 1;
                }
            }
        }
        for (slot, present) in seen.iter().enumerate() {
            if *present {
                continue;
            }
            if let Some(entry) = self.files[slot].take() {
                let start = entry.id_start as usize;
                let end = start + entry.id_len as usize;
                let id = std::str::from_utf8(&self.id_bytes[start..end])
                    .expect("validated ID")
                    .to_owned();
                self.tombstones
                    .insert(id, (entry.revision + 1, now_nanos()));
                self.changed.remove(&slot);
                self.changed_bigrams.remove(&slot);
                self.malformed.remove(&slot);
                self.presence.remove(slot);
                self.view.remove(slot);
                self.sort.remove(slot);
                self.modified_order.touch(slot, &self.files, &self.id_bytes);
                changed += 1;
            }
        }
        self.sorted.retain(|&slot| seen[slot]);
        self.modified_order
            .maybe_compact(&self.files, &self.id_bytes);
        if changed > 0 {
            self.key_summary.saturate();
            self.typed.invalidate_dynamic();
        }
        Ok(changed)
    }

    /// Updates a single path after an application edit or a future watcher
    /// event, without enumerating the directory.
    pub fn refresh_id(&mut self, id: &str) -> Result<()> {
        crate::repository::validate_id(id)?;
        self.key_summary.saturate();
        self.typed.invalidate_dynamic();
        let source = Source::open_scoped(&self.root, self.recursive)?;
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
                            let projection = projected_values(
                                &source,
                                id,
                                &stamp,
                                &[],
                                self.presence.fields(),
                                &self.view,
                                &self.sort,
                                true,
                            )?;
                            let keys = projection
                                .as_ref()
                                .map_or_else(|| self.view.missing_keys(), |p| p.view_keys.clone());
                            let sort_keys = projection
                                .as_ref()
                                .map_or_else(|| self.sort.missing_keys(), |p| p.sort_keys.clone());
                            if projection.is_none() {
                                self.malformed.insert(slot);
                            } else {
                                self.malformed.remove(&slot);
                            }
                            self.presence.update(
                                slot,
                                projection
                                    .as_ref()
                                    .map_or(&[], |p| p.present_fields.as_slice()),
                            );
                            if let Some(projection) = projection.as_ref() {
                                self.array_fields
                                    .extend(projection.array_fields.iter().cloned());
                            }
                            let entry = self.files[slot].as_mut().expect("live sorted slot");
                            entry.stamp = stamp;
                            entry.revision += 1;
                            entry.changed_at_nanos = now_nanos();
                            self.changed.insert(slot);
                            if let Some(projection) = projection.as_ref() {
                                self.changed_bigrams
                                    .insert(slot, sorted_bigrams(&projection.bytes, self.encoding));
                            } else {
                                self.changed_bigrams.remove(&slot);
                            }
                            self.view.update(slot, keys, &self.files, &self.id_bytes);
                            self.sort
                                .update(slot, sort_keys, &self.files, &self.id_bytes);
                            self.modified_order.touch(slot, &self.files, &self.id_bytes);
                        }
                    }
                    Err(_) => {
                        let projection = projected_values(
                            &source,
                            id,
                            &stamp,
                            &[],
                            self.presence.fields(),
                            &self.view,
                            &self.sort,
                            true,
                        )?;
                        let keys = projection
                            .as_ref()
                            .map_or_else(|| self.view.missing_keys(), |p| p.view_keys.clone());
                        let sort_keys = projection
                            .as_ref()
                            .map_or_else(|| self.sort.missing_keys(), |p| p.sort_keys.clone());
                        if let Some(projection) = projection.as_ref() {
                            self.array_fields
                                .extend(projection.array_fields.iter().cloned());
                        }
                        let slot = self.insert(id, stamp)?;
                        if projection.is_none() {
                            self.malformed.insert(slot);
                        }
                        self.presence.update(
                            slot,
                            projection
                                .as_ref()
                                .map_or(&[], |p| p.present_fields.as_slice()),
                        );
                        self.view.update(slot, keys, &self.files, &self.id_bytes);
                        self.sort
                            .update(slot, sort_keys, &self.files, &self.id_bytes);
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
                    self.view.remove(slot);
                    self.sort.remove(slot);
                    self.modified_order.touch(slot, &self.files, &self.id_bytes);
                }
            }
        }
        source.finish()?;
        self.modified_order
            .maybe_compact(&self.files, &self.id_bytes);
        Ok(())
    }
}
