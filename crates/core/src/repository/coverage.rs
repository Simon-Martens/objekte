//! Cache status and collection coverage reporting.

use super::*;


impl Repository {
    pub fn cache_status(&mut self) -> Result<CacheStatus> {
        self.reload()?;
        let directory = self.cache_directory()?;
        let store =
            if self.options.cache == CacheMode::Sqlite && directory.join("cache.sqlite").exists() {
                Some(self.store()?)
            } else {
                None
            };
        let guard = store
            .as_ref()
            .map(|w| w.enter(self.options.sqlite_cache_mib, self.options.sql_diagnostics))
            .transpose()?;
        let mut collections = Vec::new();
        for name in self.model.collections.keys() {
            collections.push(self.coverage(name, guard.as_deref())?);
        }
        Ok(CacheStatus {
            directory,
            mode: self.options.cache,
            freshness: self.options.freshness,
            collections,
            configuration_changes: store.as_ref().and(self.configuration_changes.clone()),
        })
    }

    pub(super) fn coverage(&self, name: &str, s: Option<&Store>) -> Result<Coverage> {
        if let Some(s) = s
            && let Some(state) = s.state(name)?
        {
            let c = self.model.collection(name)?;
            let configuration_current = state.schema == c.schema_hash
                && state.mapping == c.mapping_hash
                && (state.index.is_empty() || state.index == c.index_hash)
                && state.view == c.view_hashes.combined;
            let records = s.db.query_row(
                &format!(
                    "SELECT count(*) FROM {} WHERE collection=?1",
                    store::record_table()
                ),
                [name],
                |r| r.get::<_, i64>(0),
            )? as usize;
            let validation = s.db.prepare("SELECT validation_state,count(*) FROM records WHERE collection=?1 GROUP BY validation_state")?
                .query_map([name], |r| Ok((r.get(0)?,r.get::<_, i64>(1)? as usize)))?
                .collect::<std::result::Result<BTreeMap<_,_>,_>>()?;
            return Ok(Coverage {
                validation,
                collection: name.into(),
                verified: state.state == "current"
                    && configuration_current
                    && self.verified.get(name) == Some(&state.revision),
                configuration_current,
                applied_schema_hash: (!state.schema.is_empty()).then_some(state.schema),
                expected_schema_hash: c.schema_hash.clone(),
                state: state.state,
                records,
                revision: state.revision,
                manifest_hash: (!state.manifest.is_empty()).then_some(state.manifest),
                view_hash: (!state.view.is_empty()).then_some(state.view),
            });
        }
        let state = self
            .memory_coverage
            .get(name)
            .cloned()
            .unwrap_or_else(|| "absent".into());
        Ok(Coverage {
            collection: name.into(),
            verified: state == "current",
            configuration_current: state == "current",
            applied_schema_hash: None,
            expected_schema_hash: self.model.collection(name)?.schema_hash.clone(),
            state,
            records: self.memory_edges.keys().filter(|(n, _)| n == name).count(),
            validation: BTreeMap::new(),
            revision: 0,
            manifest_hash: None,
            view_hash: None,
        })
    }
}
