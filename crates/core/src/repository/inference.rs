//! Explicit field inference, optional ordered postings, and schema updates.

mod schema_write;

use super::*;
use crate::{
    InferOptions, InferenceReport, config::fingerprint, file_picker::ReferenceInventory,
    source::Source,
};


impl Repository {
    /// Rescan one collection, or all collections, for nested field types.
    /// Ordered postings are built for inferred booleans, numbers, and enums
    /// when `auto_index` is true. A present schema can be extended on request.
    pub fn infer_and_auto_index(
        &mut self,
        collection: Option<&str>,
        options: InferOptions,
    ) -> Result<InferenceReport> {
        let started = Instant::now();
        self.reload()?;
        let collections = if let Some(name) = collection {
            vec![self.collection(name)?]
        } else {
            self.model.collections.values().cloned().collect()
        };
        let inventory = ReferenceInventory::build(&self.model)?;
        let mut reports = Vec::with_capacity(collections.len());
        for selected in collections {
            self.ensure_picker(&selected)?;
            let picker = self
                .file_pickers
                .get_mut(&selected.name)
                .expect("picker exists");
            let mut report = picker.infer_fields(&selected.name, &inventory, options.auto_index)?;
            report.schema_snapshot = selected.schema_hash.clone();
            if options.auto_index {
                let old = self
                    .auto_postings
                    .remove(&selected.name)
                    .unwrap_or_default();
                if let Some(requested) = self.requested_postings.get_mut(&selected.name) {
                    for path in old {
                        requested.remove(&path);
                    }
                }
                let auto = picker.inferred_paths().into_iter().collect::<BTreeSet<_>>();
                self.requested_postings
                    .entry(selected.name.clone())
                    .or_default()
                    .extend(auto.iter().cloned());
                self.auto_postings.insert(selected.name.clone(), auto);
            }
            reports.push(report);
        }
        let mut report = InferenceReport {
            collections: reports,
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
            reference_snapshot: inventory.snapshot().to_owned(),
        };
        if options.write_schema {
            self.write_inferred_schema(&mut report)?;
            report.elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        }
        Ok(report)
    }

    /// Persist the rules and types from a completed scan in existing schemas.
    /// Source and schema snapshots are checked before any schema is changed.
    pub fn write_inferred_schema(&mut self, report: &mut InferenceReport) -> Result<usize> {
        self.reload()?;
        if ReferenceInventory::build(&self.model)?.snapshot() != report.reference_snapshot {
            return Err(Error::Request(
                "repository source changed since inference; rerun advanced indexing".into(),
            ));
        }
        for scanned in &report.collections {
            let current = self.collection(&scanned.collection)?;
            if current.schema_hash != scanned.schema_snapshot {
                return Err(Error::Request(format!(
                    "schema changed since inference for {}",
                    scanned.collection
                )));
            }
            let source = Source::open_scoped(&current.path, current.recursive)?;
            let inventory = source.inventory()?;
            source.finish()?;
            if fingerprint("inference-source/v1", &inventory.iter().collect::<Vec<_>>())
                != scanned.source_snapshot
            {
                return Err(Error::Request(format!(
                    "source changed since inference for {}; rerun advanced indexing",
                    scanned.collection
                )));
            }
        }
        let mut written = 0;
        for scanned in &mut report.collections {
            let current = self.collection(&scanned.collection)?;
            if let Some(path) = current.schema_path.as_deref() {
                scanned.schema_written = schema_write::extend_schema(path, scanned)?;
                written += usize::from(scanned.schema_written);
            }
        }
        if written > 0 {
            self.reload()?;
        }
        Ok(written)
    }
}
