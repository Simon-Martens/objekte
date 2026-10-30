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

    /// Reads the authoritative file.
    pub fn get_record(&mut self, collection: &str, id: &str) -> Result<Option<Record>> {
        self.get_record_inner(collection, id)
    }

    pub(super) fn get_record_inner(
        &mut self,
        collection: &str,
        id: &str,
    ) -> Result<Option<Record>> {
        self.reload()?;
        let c = self.collection(collection)?;
        validate_id(id)?;
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
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        let value = self.parse(&c, id, &raw.json, false);
        let edges = self.extract(&c, id, &value)?;
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
        if complete {
            for source in &sources {
                let c = self.collection(source)?;
                self.direct(&c, None, false)?;
            }
        }
        for source in &sources {
            coverage.push(self.coverage(source)?);
        }
        edges.extend(
            self.memory_edges
                .values()
                .flatten()
                .filter(|e| e.target_collection == collection && e.target_id == id)
                .cloned(),
        );
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
