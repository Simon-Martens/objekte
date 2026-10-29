//! Reconcile source records into published segment generations.
use super::*;


impl Repository {
    pub(in crate::repository) fn segment_config_refresh(&mut self) -> Result<()> {
        let directory = self.cache_directory()?.join("segments");
        let _guard = lock(&directory, true)?;
        let path = directory.join("CONFIG.json");
        let previous: ConfigurationSnapshot = if path.exists() {
            read_json(&path)?
        } else {
            ConfigurationSnapshot::default()
        };
        let mut changes = previous.compare(&self.model);
        changes.persistent = true;
        if !path.exists() || !changes.inputs.is_empty() {
            if self.model.changed()? {
                return Err(Error::Unstable(self.model.root.clone()));
            }
            write_json(&path, &ConfigurationSnapshot::from_model(&self.model))?;
        }
        self.observed_configuration = ConfigurationSnapshot::from_model(&self.model);
        self.configuration_changes = Some(changes);
        Ok(())
    }

    pub(in crate::repository) fn segment_snapshot(
        &self,
        c: &Collection,
    ) -> Result<Option<Snapshot>> {
        let directory = cache_dir(self, c)?;
        if !directory.exists() {
            return Ok(None);
        }
        let guard = lock(&directory, false)?;
        let path = directory.join("CURRENT.json");
        if !path.exists() {
            return Ok(None);
        }
        let manifest: Manifest = read_json(&path)?;
        let mut memory = self
            .segment_memory
            .lock()
            .map_err(|_| Error::Cache("segment memory poisoned".into()))?;
        let shards = if manifest.version != VERSION {
            Arc::new(Vec::new())
        } else if let Some((generation, shards)) = memory.get(&c.name)
            && *generation == manifest.generation
        {
            shards.clone()
        } else {
            let shards = Arc::new(
                manifest
                    .shard_metadata
                    .par_iter()
                    .map(|name| read_bin(&directory.join(name)))
                    .collect::<Result<Vec<_>>>()?,
            );
            memory.insert(c.name.clone(), (manifest.generation, shards.clone()));
            shards
        };
        Ok(Some(Snapshot {
            manifest,
            shards,
            directory,
            _lock: guard,
        }))
    }

    /// Return the current or deleted record revision from the selected cache.
    pub fn record_revision(
        &mut self,
        collection: &str,
        id: &str,
    ) -> Result<Option<RecordRevision>> {
        self.reload()?;
        let c = self.collection(collection)?;
        validate_id(id)?;
        match self.options.cache {
            CacheMode::Disabled => return Ok(None),
            CacheMode::Memory => {
                self.ensure_picker(&c)?;
                return Ok(self.file_pickers[collection].revision_record(id).map(
                    |(revision, changed_at_nanos, present)| RecordRevision {
                        collection: collection.into(),
                        id: id.into(),
                        revision,
                        changed_at_nanos,
                        present,
                    },
                ));
            }
            CacheMode::Segments => {}
        }
        self.sync_segments(&c, false)?;
        Ok(self
            .segment_snapshot(&c)?
            .and_then(|s| s.revision(collection, id)))
    }

    pub(in crate::repository) fn sync_segments(
        &mut self,
        c: &Collection,
        force: bool,
    ) -> Result<()> {
        self.segment_config_refresh()?;
        let directory = cache_dir(self, c)?;
        let guard = lock(&directory, true)?;
        let on_open = matches!(
            self.options.freshness,
            Freshness::MetadataOnOpen | Freshness::HashOnOpen
        );
        if !force
            && on_open
            && let Some(verified) = self.verified.get(&c.name)
            && directory.join("CURRENT.json").exists()
            && let Ok(manifest) = read_json::<Manifest>(&directory.join("CURRENT.json"))
            && manifest.version == VERSION
            && manifest.schema_hash == c.schema_hash
            && manifest.mapping_hash == c.mapping_hash
            && manifest.revision == *verified
        {
            return Ok(());
        }
        let metadata_started = Instant::now();
        let old = match load(
            &directory,
            guard.try_clone().map_err(|e| io(&directory, e))?,
        ) {
            Ok(snapshot) => snapshot,
            // The cache is derived data. A damaged or incomplete snapshot is
            // rebuilt from authoritative source files under the same lock.
            Err(Error::Cache(_)) => None,
            Err(Error::Io { ref source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
                None
            }
            Err(error) => return Err(error),
        };
        self.metrics.segment_metadata_nanos += nanos(metadata_started);
        let valid = old.as_ref().is_some_and(|snapshot| {
            snapshot.manifest.version == VERSION
                && snapshot.manifest.schema_hash == c.schema_hash
                && snapshot.manifest.mapping_hash == c.mapping_hash
        });
        let (source, files) = self.inventory(c)?;
        let source_hash = fingerprint("segment-source/v1", &files);
        if valid
            && !force
            && matches!(
                self.options.freshness,
                Freshness::MetadataOnOpen | Freshness::MetadataOnRequest
            )
            && old
                .as_ref()
                .is_some_and(|snapshot| snapshot.manifest.source_hash == source_hash)
        {
            self.metrics.manifest_hits += 1;
            self.metrics.rows_reused += files.len() as u64;
            let snapshot = old.as_ref().unwrap();
            self.segment_memory
                .lock()
                .map_err(|_| Error::Cache("segment memory poisoned".into()))?
                .insert(
                    c.name.clone(),
                    (snapshot.manifest.generation, snapshot.shards.clone()),
                );
            self.verified
                .insert(c.name.clone(), snapshot.manifest.revision);
            source.finish()?;
            return Ok(());
        }

        let count = old.as_ref().filter(|_| valid).map_or_else(
            || {
                (rayon::current_num_threads() * 2)
                    .max(1)
                    .min(files.len().max(1))
            },
            |snapshot| snapshot.shards.len(),
        );
        let generation = old
            .as_ref()
            .map_or(0, |snapshot| snapshot.manifest.generation)
            + 1;
        let mut shards = if valid {
            old.as_ref().unwrap().shards.as_ref().clone()
        } else {
            (0..count)
                .map(|i| Shard {
                    text_file: format!("text-{i:04}-{generation}.txt"),
                    text_len: 0,
                    live_bytes: 0,
                    lines: Vec::new(),
                    entries: BTreeMap::new(),
                    tombstones: BTreeMap::new(),
                    array_fields: BTreeSet::new(),
                    ordered: BTreeMap::new(),
                })
                .collect::<Vec<_>>()
        };
        if !valid {
            for shard in &shards {
                let path = directory.join(&shard.text_file);
                File::create(&path).map_err(|e| io(&path, e))?;
            }
        }
        let mut touched = vec![!valid; count];
        let mut writers: Vec<Option<BufWriter<File>>> = (0..count).map(|_| None).collect();
        let fields = ordered_fields(c);
        let metadata_mode = matches!(
            self.options.freshness,
            Freshness::MetadataOnOpen | Freshness::MetadataOnRequest
        );
        for (id, stamp) in &files {
            let index = shard_number(id, count);
            let shard = &mut shards[index];
            let previous = shard.entries.get(id).cloned();
            if valid && metadata_mode && previous.as_ref().is_some_and(|e| &e.stamp == stamp) {
                self.metrics.metadata_reused += 1;
                self.metrics.rows_reused += 1;
                continue;
            }
            let raw = self.raw_from(&source, id, Some(stamp))?;
            if valid && previous.as_ref().is_some_and(|e| e.digest == raw.hash) {
                shard.entries.get_mut(id).unwrap().stamp = raw.stamp;
                touched[index] = true;
                self.metrics.rows_reused += 1;
                continue;
            }
            let doc = self.parse(c, id, &raw.json, false);
            let mut text = String::new();
            if doc.state.status != RecordStatus::IncorrectJson {
                crate::json::indexed_value_text(&doc.data, &mut text);
            }
            text.push('\n');
            if let Some(previous) = &previous {
                shard.lines[previous.line] = None;
                shard.live_bytes -= previous.text_len;
            }
            if writers[index].is_none() {
                let path = directory.join(&shard.text_file);
                let mut file = OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(false)
                    .open(&path)
                    .map_err(|e| io(&path, e))?;
                file.set_len(shard.text_len).map_err(|e| io(&path, e))?;
                use std::io::{Seek, SeekFrom};
                file.seek(SeekFrom::Start(shard.text_len))
                    .map_err(|e| io(&path, e))?;
                writers[index] = Some(BufWriter::new(file));
            }
            let writer = writers[index].as_mut().unwrap();
            writer
                .write_all(text.as_bytes())
                .map_err(|e| io(&directory, e))?;
            let mut keys = BTreeMap::new();
            for field in &fields {
                keys.insert(
                    field.clone(),
                    crate::query::scalar(id, &doc.data, field, c.fields.get(field)),
                );
            }
            let revived = shard.tombstones.remove(id);
            let revision = previous.as_ref().map_or_else(
                || revived.as_ref().map_or(1, |entry| entry.revision + 1),
                |entry| entry.revision + 1,
            );
            let entry = Entry {
                changed_at_nanos: if previous.is_some() || revived.is_some() {
                    now_nanos()
                } else {
                    raw.stamp.modified.parse().unwrap_or(0)
                },
                stamp: raw.stamp,
                digest: raw.hash,
                revision,
                line: shard.lines.len(),
                offset: shard.text_len,
                text_len: text.len() as u64,
                keys,
                array_fields: doc.data.as_object().map_or_else(BTreeSet::new, |object| {
                    object
                        .iter()
                        .filter(|(_, value)| value.is_array())
                        .map(|(name, _)| name.clone())
                        .collect()
                }),
            };
            shard.text_len += entry.text_len;
            shard.live_bytes += entry.text_len;
            shard.lines.push(Some(id.clone()));
            shard.entries.insert(id.clone(), entry);
            touched[index] = true;
            if previous.is_some() {
                self.metrics.rows_updated += 1;
            } else {
                self.metrics.rows_inserted += 1;
            }
        }
        for (index, shard) in shards.iter_mut().enumerate() {
            let removed = shard
                .entries
                .keys()
                .filter(|id| !files.contains_key(*id))
                .cloned()
                .collect::<Vec<_>>();
            for id in removed {
                let entry = shard.entries.remove(&id).unwrap();
                shard.lines[entry.line] = None;
                shard.live_bytes -= entry.text_len;
                shard.tombstones.insert(
                    id,
                    Tombstone {
                        revision: entry.revision + 1,
                        changed_at_nanos: now_nanos(),
                    },
                );
                touched[index] = true;
                self.metrics.rows_deleted += 1;
            }
        }
        for writer in writers.into_iter().flatten() {
            let mut writer = writer;
            writer.flush().map_err(|e| io(&directory, e))?;
            writer.get_ref().sync_all().map_err(|e| io(&directory, e))?;
        }
        source.finish()?;
        if !metadata_mode && self.inventory(c)?.1 != files {
            return Err(Error::Unstable(c.path.clone()));
        }
        if self.model.changed()? {
            return Err(Error::Unstable(self.model.root.clone()));
        }
        let changed = !valid || touched.iter().any(|v| *v);
        let revision = old
            .as_ref()
            .map_or(0, |snapshot| snapshot.manifest.revision)
            + i64::from(changed);
        let mut shard_metadata = old.as_ref().filter(|_| valid).map_or_else(
            || vec![String::new(); count],
            |snapshot| snapshot.manifest.shard_metadata.clone(),
        );
        for (index, shard) in shards.iter_mut().enumerate() {
            if !touched[index] {
                continue;
            }
            if shard.text_len > 0
                && shard.text_len.saturating_sub(shard.live_bytes) * 100
                    > shard.live_bytes * COMPACT_STALE_PERCENT
            {
                let old_path = directory.join(&shard.text_file);
                let old_bytes = fs::read(&old_path).map_err(|e| io(&old_path, e))?;
                let new_name = format!("text-{index:04}-{generation}.txt");
                let new_path = directory.join(&new_name);
                let mut writer =
                    BufWriter::new(File::create(&new_path).map_err(|e| io(&new_path, e))?);
                let mut offset = 0;
                let mut lines = Vec::new();
                for (id, entry) in &mut shard.entries {
                    let start = entry.offset as usize;
                    let end = start + entry.text_len as usize;
                    writer
                        .write_all(&old_bytes[start..end])
                        .map_err(|e| io(&new_path, e))?;
                    entry.offset = offset;
                    entry.line = lines.len();
                    lines.push(Some(id.clone()));
                    offset += entry.text_len;
                }
                writer.flush().map_err(|e| io(&new_path, e))?;
                writer.get_ref().sync_all().map_err(|e| io(&new_path, e))?;
                shard.text_file = new_name;
                shard.text_len = offset;
                shard.lines = lines;
            }
            rebuild_ordered(shard, &fields);
            self.metrics.index_builds += fields.len() as u64;
            self.metrics.index_rows_written += (fields.len() * shard.entries.len()) as u64;
            let name = format!("meta-{index:04}-{generation}.bin");
            write_bin(&directory.join(&name), shard)?;
            shard_metadata[index] = name;
        }
        let manifest = Manifest {
            version: VERSION,
            schema_hash: c.schema_hash.clone(),
            mapping_hash: c.mapping_hash.clone(),
            revision,
            source_hash,
            generation,
            shard_metadata,
        };
        write_json(&directory.join("CURRENT.json"), &manifest)?;
        self.segment_memory
            .lock()
            .map_err(|_| Error::Cache("segment memory poisoned".into()))?
            .insert(c.name.clone(), (generation, Arc::new(shards.clone())));
        let keep = manifest
            .shard_metadata
            .iter()
            .chain(shards.iter().map(|shard| &shard.text_file))
            .collect::<BTreeSet<_>>();
        for item in fs::read_dir(&directory).map_err(|e| io(&directory, e))? {
            let item = item.map_err(|e| io(&directory, e))?;
            let name = item.file_name().to_string_lossy().into_owned();
            if (name.starts_with("meta-") || name.starts_with("text-"))
                && !keep.iter().any(|kept| kept.as_str() == name)
            {
                let _ = fs::remove_file(item.path());
            }
        }
        self.verified.insert(c.name.clone(), revision);
        self.metrics.reconciliations += 1;
        Ok(())
    }
}
