//! Configuration snapshots and changed-input reports for one repository handle.
use crate::RepositoryModel;
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};


#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigurationChangeKind {
    Added,
    Modified,
    Removed,
}


// INFO: One changed configuration file and its affected collections.
#[derive(Debug, Clone, Serialize)]
pub struct ConfigurationInputChange {
    /// Path relative to the repository root.
    pub path: PathBuf,
    pub kind: ConfigurationChangeKind,
    pub previous_digest: Option<String>,
    pub digest: Option<String>,
    /// Direct consumers of this input, including former consumers of removed files.
    pub collections: Vec<String>,
}


// INFO: Changed inputs and their candidate collections.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ConfigurationChanges {
    pub inputs: Vec<ConfigurationInputChange>,
    /// Candidate collections; semantic hashes decide which index work is necessary.
    pub affected_collections: Vec<String>,
}


// INFO: Configuration file digests and their direct collection owners.
#[derive(Default, Clone)]
pub(crate) struct ConfigurationSnapshot {
    files: BTreeMap<PathBuf, String>,
    owners: BTreeMap<PathBuf, BTreeSet<String>>,
    collections: BTreeSet<String>,
}


impl ConfigurationSnapshot {
    pub fn from_model(model: &RepositoryModel) -> Self {
        Self {
            files: model.files.clone(),
            owners: model.input_collections.clone(),
            collections: model.collections.keys().cloned().collect(),
        }
    }

    pub fn compare(&self, model: &RepositoryModel) -> ConfigurationChanges {
        let mut report = ConfigurationChanges::default();
        let mut affected = BTreeSet::new();
        let current = model.collections.keys().cloned().collect::<BTreeSet<_>>();
        affected.extend(self.collections.symmetric_difference(&current).cloned());
        for path in self
            .files
            .keys()
            .chain(model.files.keys())
            .collect::<BTreeSet<_>>()
        {
            let old = self.files.get(path);
            let new = model.files.get(path);
            if old == new {
                continue;
            }
            let mut owners = model
                .input_collections
                .get(path)
                .cloned()
                .unwrap_or_default();
            if let Some(previous) = self.owners.get(path) {
                owners.extend(previous.iter().cloned());
            }
            affected.extend(owners.iter().cloned());
            report.inputs.push(ConfigurationInputChange {
                path: path.strip_prefix(&model.root).unwrap_or(path).into(),
                kind: match (old, new) {
                    (None, _) => ConfigurationChangeKind::Added,
                    (_, None) => ConfigurationChangeKind::Removed,
                    _ => ConfigurationChangeKind::Modified,
                },
                previous_digest: old.cloned(),
                digest: new.cloned(),
                collections: owners.into_iter().collect(),
            });
        }
        report.affected_collections = affected.into_iter().collect();
        report
    }
}
