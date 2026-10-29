//! Observed configuration is independent of the versions applied to cached records.
use crate::{Error, RepositoryModel, Result, store::Store};
use rusqlite::params;
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

#[derive(Debug, Clone, Default, Serialize)]
pub struct ConfigurationChanges {
    /// False when comparing only an in-memory snapshot with caching disabled.
    pub persistent: bool,
    pub inputs: Vec<ConfigurationInputChange>,
    /// Candidate collections; semantic hashes decide which cache work is necessary.
    pub affected_collections: Vec<String>,
}

#[derive(Default, Clone)]
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
    pub fn compare(
        &self,
        model: &RepositoryModel,
        fallback: &BTreeSet<String>,
    ) -> ConfigurationChanges {
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
            } else if old.is_some() {
                // Older caches stored digests without consumers. Be conservative once.
                owners.extend(fallback.iter().cloned());
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

impl Store {
    /// Called under the shared writer lock. Digest and ownership changes commit together;
    /// collection/record validation versions advance only in their own reconciliation.
    pub fn reconcile_configuration(&self, model: &RepositoryModel) -> Result<ConfigurationChanges> {
        let tx = self.db.unchecked_transaction()?;
        let files = tx
            .prepare("SELECT path,digest FROM inputs")?
            .query_map([], |r| {
                Ok((PathBuf::from(r.get::<_, String>(0)?), r.get(1)?))
            })?
            .collect::<std::result::Result<BTreeMap<_, _>, _>>()?;
        let pairs = tx
            .prepare("SELECT path,collection FROM input_collections")?
            .query_map([], |r| {
                Ok((
                    PathBuf::from(r.get::<_, String>(0)?),
                    r.get::<_, String>(1)?,
                ))
            })?
            .collect::<std::result::Result<BTreeSet<_>, _>>()?;
        let mut owners = BTreeMap::<_, BTreeSet<_>>::new();
        for (path, name) in &pairs {
            owners.entry(path.clone()).or_default().insert(name.clone());
        }
        let mut fallback = tx
            .prepare("SELECT name FROM collections")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<BTreeSet<_>, _>>()?;
        fallback.extend(model.collections.keys().cloned());
        let previous = ConfigurationSnapshot { files, owners };
        let mut report = previous.compare(model, &fallback);
        report.persistent = true;
        for change in &report.inputs {
            let path = model.root.join(&change.path);
            match &change.digest {
                Some(hash) => {
                    tx.execute("INSERT INTO inputs(path,digest) VALUES(?1,?2) ON CONFLICT(path) DO UPDATE SET digest=excluded.digest", params![path.to_str().unwrap(), hash])?;
                }
                None => {
                    tx.execute("DELETE FROM inputs WHERE path=?1", [path.to_str().unwrap()])?;
                }
            }
        }
        let current_pairs = model
            .input_collections
            .iter()
            .flat_map(|(p, names)| names.iter().map(|n| (p.clone(), n.clone())))
            .collect::<BTreeSet<_>>();
        for (p, n) in pairs.difference(&current_pairs) {
            tx.execute(
                "DELETE FROM input_collections WHERE path=?1 AND collection=?2",
                params![p.to_str().unwrap(), n],
            )?;
        }
        for (p, n) in current_pairs.difference(&pairs) {
            tx.execute(
                "INSERT INTO input_collections(path,collection) VALUES(?1,?2)",
                params![p.to_str().unwrap(), n],
            )?;
        }
        // A handle loaded before another writer must not publish its obsolete snapshot.
        if model.changed()? {
            return Err(Error::Unstable(model.root.clone()));
        }
        tx.commit()?;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_compiled_model_cannot_replace_a_newer_persisted_manifest() {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/minimal");
        let current = RepositoryModel::load(&root).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path(), &current.root, None, false).unwrap();
        store.reconcile_configuration(&current).unwrap();
        let path = current.files.keys().next().unwrap();
        let mut obsolete = current.clone();
        obsolete
            .files
            .insert(path.clone(), "obsolete-digest".into());
        assert!(matches!(
            store.reconcile_configuration(&obsolete),
            Err(Error::Unstable(_))
        ));
        let saved: String = store
            .db
            .query_row(
                "SELECT digest FROM inputs WHERE path=?1",
                [path.to_str().unwrap()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(saved, current.files[path]);
        assert!(
            store
                .reconcile_configuration(&current)
                .unwrap()
                .inputs
                .is_empty()
        );
    }
}
