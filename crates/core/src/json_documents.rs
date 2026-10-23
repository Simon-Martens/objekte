//! Schema-bound JSON files are independent of collection records and UI controls.
use crate::documents::{atomic_write, hash, record_id, reject_symlink};
use crate::{Document, Editor, Error, Result, ValidationIssue, contained};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};

impl Editor {
    pub fn document_schema(&self, name: &str) -> Result<&Value> {
        self.config()
            .documents
            .get(name)
            .map(|config| &config.schema)
            .ok_or_else(|| Error::NotFound(format!("document mapping {name}")))
    }

    pub fn document_files(&self, name: &str) -> Result<&[String]> {
        self.config()
            .documents
            .get(name)
            .map(|config| config.files.as_slice())
            .ok_or_else(|| Error::NotFound(format!("document mapping {name}")))
    }

    fn document_path(&self, name: &str, file: &str) -> Result<PathBuf> {
        if !self.document_files(name)?.iter().any(|path| path == file) {
            return Err(Error::NotFound(format!("{file} is not assigned to {name}")));
        }
        let path = self.config().root.join(file);
        reject_symlink(&path)?;
        contained(&self.config().root, file)
    }

    pub fn json_document(&self, name: &str, file: &str) -> Result<Document> {
        let bytes = fs::read(self.document_path(name, file)?)?;
        let value =
            serde_json::from_slice(&bytes).map_err(|e| Error::Data(format!("{file}: {e}")))?;
        Ok(Document {
            id: file.into(),
            value,
            revision: hash(&bytes),
        })
    }

    pub fn validate_document(&self, name: &str, value: &Value) -> Result<Vec<ValidationIssue>> {
        let validator = jsonschema::draft202012::options()
            .offline()
            .should_validate_formats(true)
            .build(self.document_schema(name)?)
            .map_err(|e| Error::Config(format!("{name}: {e}")))?;
        let mut issues: Vec<ValidationIssue> = validator
            .iter_errors(value)
            .map(|e| ValidationIssue {
                path: e.instance_path().to_string(),
                message: e.to_string(),
            })
            .collect();
        issues.extend(self.validate_reference_values(self.document_schema(name)?, value)?);
        Ok(issues)
    }

    pub fn save_json_document(
        &self,
        name: &str,
        file: &str,
        value: Value,
        expected: &str,
    ) -> Result<Document> {
        let path = self.document_path(name, file)?;
        let issues = self.validate_document(name, &value)?;
        if !issues.is_empty() {
            return Err(Error::Validation(issues));
        }
        // Alternate access to a managed file must retain its own write rules.
        if let Some(settings) = &self.config().settings {
            if path == contained(&self.config().root, &settings.path)? {
                let saved = self.save_settings(value, expected)?;
                return Ok(Document {
                    id: file.into(),
                    ..saved
                });
            }
        }
        for (collection, config) in &self.config().collections {
            if config.format == "json"
                && path.parent() == Some(self.config().collection_dir(collection)?.as_path())
            {
                let current = self.json_document(name, file)?;
                let id = record_id(config, &path, &current.value)?;
                let saved = self.save(collection, &id, value, Some(expected))?;
                return Ok(Document {
                    id: file.into(),
                    ..saved
                });
            }
        }
        let _guard = self.write_guard.lock().unwrap();
        // Check targets again under the same lock as record deletion/rename.
        let issues = self.validate_document(name, &value)?;
        if !issues.is_empty() {
            return Err(Error::Validation(issues));
        }
        let current = self.json_document(name, file)?;
        if current.revision != expected {
            return Err(Error::Conflict(format!("{file} changed on disk")));
        }
        let mut bytes =
            serde_json::to_vec_pretty(&value).map_err(|e| Error::Data(e.to_string()))?;
        bytes.push(b'\n');
        atomic_write(&self.document_path(name, file)?, &bytes)?;
        Ok(Document {
            id: file.into(),
            value,
            revision: hash(&bytes),
        })
    }

    pub(crate) fn mapped_document_name(&self, path: &Path) -> Option<&str> {
        self.config()
            .documents
            .iter()
            .find(|(_, mapping)| {
                mapping
                    .files
                    .iter()
                    .any(|file| contained(&self.config().root, file).ok().as_deref() == Some(path))
            })
            .map(|(name, _)| name.as_str())
    }

    pub(crate) fn guard_document_location(&self, path: &Path) -> Result<()> {
        if self
            .config()
            .documents
            .values()
            .flat_map(|m| &m.files)
            .any(|file| {
                contained(&self.config().root, file).is_ok_and(|file| file.starts_with(path))
            })
        {
            return Err(Error::Conflict("path contains a configured document; update its assignment before moving or deleting it".into()));
        }

        if self.mapped_document_name(path).is_some()
            || self.config().settings.as_ref().is_some_and(|s| {
                contained(&self.config().root, &s.path).ok().as_deref() == Some(path)
            })
        {
            return Err(Error::Conflict("file is assigned in repository configuration; update that assignment before moving or deleting it".into()));
        }
        Ok(())
    }
}
