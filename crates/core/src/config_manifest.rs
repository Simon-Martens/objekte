//! Observed configuration is independent of the versions applied to cached records.
use crate::RepositoryModel;
use serde::{Deserialize, Serialize};
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


// INFO: Persisted or in-memory summary of changed inputs and candidate collections.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ConfigurationChanges {
    /// False when comparing only an in-memory snapshot with caching disabled.
    pub persistent: bool,
    pub inputs: Vec<ConfigurationInputChange>,
    /// Candidate collections; semantic hashes decide which cache work is necessary.
    pub affected_collections: Vec<String>,
}


// INFO: Configuration file digests and their direct collection owners. Saved in the segment configuration snapshot.
#[derive(Default, Clone, Serialize, Deserialize)]
pub(crate) struct ConfigurationSnapshot {
    files: BTreeMap<PathBuf, String>,
    owners: BTreeMap<PathBuf, BTreeSet<String>>,
}


impl ConfigurationSnapshot {
    pub fn from_model(model: &RepositoryModel) -> Self {
        Self {
            files: model.files.clone(),
            owners: model.input_collections.clone(),
        }
    }

    pub fn compare(&self, model: &RepositoryModel) -> ConfigurationChanges {
        let mut report = ConfigurationChanges::default();
        let mut affected = BTreeSet::new();
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
