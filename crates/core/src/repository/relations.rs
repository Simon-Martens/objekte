//! Reference extraction and lookup, including partial coverage.

use super::*;

impl Repository {
    pub(super) fn path(&self, c: &Collection, id: &str) -> String {
        c.path
            .join(format!("{id}.json"))
            .strip_prefix(&self.model.root)
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    pub(super) fn extract(
        &self,
        c: &Collection,
        id: &str,
        data: &Value,
    ) -> Result<Vec<RecordRelation>> {
        fn visit(value: &Value, path: &[PathStep], at: &str, out: &mut Vec<(String, String)>) {
            match path.split_first() {
                None => {
                    if let Some(id) = value.as_str().filter(|s| !s.is_empty()) {
                        out.push((at.into(), id.into()))
                    }
                }
                Some((PathStep::Field(name), rest)) => {
                    if let Some(v) = value.get(name) {
                        visit(v, rest, &format!("{at}/{}", escape(name)), out)
                    }
                }
                Some((PathStep::Each, rest)) => {
                    if let Some(values) = value.as_array() {
                        for (i, v) in values.iter().enumerate() {
                            visit(v, rest, &format!("{at}/{i}"), out)
                        }
                    }
                }
            }
        }
        let mut out = Vec::new();
        for edge in self.model.graph.edges.iter().filter(|e| e.source == c.name) {
            let mut values = Vec::new();
            visit(data, &edge.path, "", &mut values);
            for (location, target_id) in values {
                if validate_id(&target_id).is_err() {
                    continue;
                }
                out.push(RecordRelation {
                    source_collection: c.name.clone(),
                    source_id: id.into(),
                    location,
                    target_collection: edge.target.clone(),
                    target_id,
                    target_state: TargetState::Unknown,
                });
            }
        }
        out.sort_by(|a, b| {
            (&a.location, &a.target_collection, &a.target_id).cmp(&(
                &b.location,
                &b.target_collection,
                &b.target_id,
            ))
        });
        Ok(out)
    }

    /// Reads the authoritative file, even when a persistent cache exists.
    pub fn get_record(&mut self, collection: &str, id: &str) -> Result<Option<Record>> {
        let before = self.metrics.clone();
        let result = self.get_record_inner(collection, id);
        if result.is_err() {
            self.metrics.rollback_writes(&before);
        }
        result
    }

    pub(super) fn get_record_inner(
        &mut self,
        collection: &str,
        id: &str,
    ) -> Result<Option<Record>> {
        self.reload()?;
        let c = self.collection(collection)?;
        validate_id(id)?;
        self.verified.remove(collection);
        self.memory_coverage
            .insert(collection.into(), "partial".into());
        let result = self.raw(&c, id);
        let raw = match result {
            Ok(v) => v,
            Err(Error::Io { path, source })
                if source.kind() == std::io::ErrorKind::NotFound
                    && path == c.path.join(format!("{id}.json")) =>
            {
                self.memory_edges.remove(&(collection.into(), id.into()));
                if self.options.cache == CacheMode::Sqlite {
                    let writer = self.store()?;
                    let s = writer
                        .enter(self.options.sqlite_cache_mib, self.options.sql_diagnostics)?;
                    s.dirty(collection)?;
                    let tx = s.db.unchecked_transaction()?;
                    store::delete_record(&tx, &c, id, &mut self.metrics)?;
                    tx.commit()?;
                }
                return Ok(None);
            }
            Err(e) => {
                if self.options.cache == CacheMode::Sqlite
                    && let Ok(writer) = self.store()
                    && let Ok(s) =
                        writer.enter(self.options.sqlite_cache_mib, self.options.sql_diagnostics)
                {
                    let _ = s.dirty(collection);
                }
                return Err(e);
            }
        };
        let mut value = self.parse(&c, id, &raw.json, self.options.cache == CacheMode::Disabled);
        let edges = self.extract(&c, id, &value)?;
        if self.options.cache == CacheMode::Sqlite {
            let writer = self.store()?;
            let s = writer.enter(self.options.sqlite_cache_mib, self.options.sql_diagnostics)?;
            let old_state = s.state(collection)?;
            s.dirty(collection)?;
            let tx = s.db.unchecked_transaction()?;
            self.put_record(
                &tx,
                &c,
                id,
                &raw,
                &value,
                (old_state
                    .as_ref()
                    .is_some_and(|v| v.index == c.index_hash && v.schema == c.schema_hash)
                    && s.has_indexes(&c)?)
                .then_some(&s.index_cache),
            )?;
            if old_state.as_ref().is_none_or(|s| s.state == "partial") {
                tx.execute(
                    "UPDATE collections SET state='partial' WHERE name=?1",
                    [collection],
                )?;
            }
            tx.commit()?;
            value.state = cached_state(&s.db, collection, id)?;
        }
        self.schedule_validation(&c)?;
        self.memory_edges
            .insert((collection.into(), id.into()), edges);
        Ok(Some(Record {
            collection: collection.into(),
            id: id.into(),
            path: self.path(&c, id),
            hash: raw.hash,
            raw: (value.state.status == RecordStatus::IncorrectJson).then_some(raw.json),
            state: value.state,
            data: value.data,
        }))
    }

    pub fn references_from(
        &mut self,
        collection: &str,
        id: &str,
        check_targets: bool,
    ) -> Result<References> {
        self.get_record(collection, id)?;
        let mut edges = self
            .memory_edges
            .get(&(collection.into(), id.into()))
            .cloned()
            .unwrap_or_default();
        if check_targets {
            self.resolve_targets(&mut edges)?;
        }
        let coverage = self
            .cache_status()?
            .collections
            .into_iter()
            .filter(|c| c.collection == collection)
            .collect();
        Ok(References { edges, coverage })
    }

    pub fn references_to(
        &mut self,
        collection: &str,
        id: &str,
        complete: bool,
        check_targets: bool,
    ) -> Result<References> {
        self.reload()?;
        self.model.collection(collection)?;
        validate_id(id)?;
        let sources = self
            .model
            .graph
            .edges
            .iter()
            .filter(|e| e.target == collection)
            .map(|e| e.source.clone())
            .collect::<BTreeSet<_>>();
        let mut edges = Vec::new();
        let mut coverage = Vec::new();
        if self.options.cache == CacheMode::Sqlite {
            let writer = self.store()?;
            let s = writer.enter(self.options.sqlite_cache_mib, self.options.sql_diagnostics)?;
            if complete {
                for source in &sources {
                    let c = self.collection(source)?;
                    self.sync(&c, &s, &[], false)?;
                }
            }
            for source in &sources {
                let mut stmt=s.db.prepare("SELECT source_collection,source_id,location,target_collection,target_id FROM edges WHERE target_collection=?1 AND target_id=?2 AND source_collection=?3 ORDER BY source_collection,source_id,location")?;
                edges.extend(
                    stmt.query_map(params![collection, id, source], |r| {
                        Ok(RecordRelation {
                            source_collection: r.get(0)?,
                            source_id: r.get(1)?,
                            location: r.get(2)?,
                            target_collection: r.get(3)?,
                            target_id: r.get(4)?,
                            target_state: TargetState::Unknown,
                        })
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?,
                );
                coverage.push(self.coverage(source, Some(&s))?);
            }
        } else {
            if complete {
                for source in &sources {
                    let c = self.collection(source)?;
                    self.direct(&c, None)?;
                }
            }
            for source in &sources {
                coverage.push(self.coverage(source, None)?);
            }
            edges.extend(
                self.memory_edges
                    .values()
                    .flatten()
                    .filter(|e| e.target_collection == collection && e.target_id == id)
                    .cloned(),
            );
        }
        if complete {
            for source in &sources {
                let c = self.collection(source)?;
                self.schedule_validation(&c)?;
            }
        }
        if check_targets {
            self.resolve_targets(&mut edges)?;
        }
        Ok(References { edges, coverage })
    }

    pub(super) fn resolve_targets(&self, edges: &mut [RecordRelation]) -> Result<()> {
        let mut known = BTreeMap::new();
        for edge in edges {
            let key = (edge.target_collection.clone(), edge.target_id.clone());
            edge.target_state = if let Some(state) = known.get(&key) {
                *state
            } else {
                let c = self.model.collection(&edge.target_collection)?;
                self.check_source(c)?;
                let p = c.path.join(format!("{}.json", edge.target_id));
                let state = match fs::symlink_metadata(&p) {
                    Ok(m) if m.file_type().is_file() => TargetState::Present,
                    Ok(_) => return Err(record(&p, "", "reference target must be a regular file")),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => TargetState::Missing,
                    Err(e) => return Err(io(&p, e)),
                };
                known.insert(key, state);
                state
            };
        }
        Ok(())
    }
}
