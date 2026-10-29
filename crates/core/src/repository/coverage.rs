//! Cache status and collection coverage reporting.

use super::*;


impl Repository {
    pub fn cache_status(&mut self) -> Result<CacheStatus> {
        self.reload()?;
        let directory = self.cache_directory()?;
        if self.options.cache == CacheMode::Segments {
            if directory.join("segments").exists() {
                self.segment_config_refresh()?;
            }
            let mut collections = Vec::new();
            for name in self.model.collections.keys() {
                let c = self.model.collection(name)?;
                let snapshot = self.segment_snapshot(c)?;
                let coverage = if let Some(snapshot) = snapshot {
                    let configuration_current = snapshot.manifest.schema_hash == c.schema_hash
                        && snapshot.manifest.mapping_hash == c.mapping_hash;
                    Coverage {
                        collection: name.clone(),
                        state: if configuration_current {
                            "current"
                        } else {
                            "dirty"
                        }
                        .into(),
                        verified: configuration_current
                            && self.verified.get(name) == Some(&snapshot.manifest.revision),
                        configuration_current,
                        applied_schema_hash: Some(snapshot.manifest.schema_hash.clone()),
                        expected_schema_hash: c.schema_hash.clone(),
                        records: snapshot.count(),
                        validation: BTreeMap::new(),
                        revision: snapshot.manifest.revision,
                        manifest_hash: Some(snapshot.manifest.source_hash.clone()),
                        view_hash: Some(c.view_hashes.combined.clone()),
                    }
                } else {
                    self.coverage(name)?
                };
                collections.push(coverage);
            }
            return Ok(CacheStatus {
                directory,
                mode: self.options.cache,
                freshness: self.options.freshness,
                collections,
                configuration_changes: self.configuration_changes.clone(),
            });
        }
        let mut collections = Vec::new();
        for name in self.model.collections.keys() {
            collections.push(self.coverage(name)?);
        }
        Ok(CacheStatus {
            directory,
            mode: self.options.cache,
            freshness: self.options.freshness,
            collections,
            configuration_changes: self.configuration_changes.clone(),
        })
    }

    pub(super) fn coverage(&self, name: &str) -> Result<Coverage> {
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
            records: self.file_pickers.get(name).map_or_else(
                || self.memory_edges.keys().filter(|(n, _)| n == name).count(),
                crate::file_picker::FilePicker::len,
            ),
            validation: BTreeMap::new(),
            revision: 0,
            manifest_hash: None,
            view_hash: None,
        })
    }
}
