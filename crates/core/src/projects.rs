//! Server-instance project registration. Repository schemas stay inside each project.
use crate::{Error, Result};
use serde::Deserialize;
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Projects {
    pub default: String,
    pub projects: Vec<Project>,
}
impl Projects {
    pub fn load(path: &Path) -> Result<Self> {
        let mut catalog: Self = serde_json::from_slice(&fs::read(path)?)
            .map_err(|e| Error::Config(format!("projects: {e}")))?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        let mut ids = HashSet::new();
        let mut roots: Vec<PathBuf> = Vec::new();
        for project in &mut catalog.projects {
            if project.id.is_empty()
                || !project
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                || !ids.insert(project.id.clone())
                || project.name.trim().is_empty()
            {
                return Err(Error::Config(
                    "projects need unique URL-safe IDs and nonempty names".into(),
                ));
            }
            project.path = base.join(&project.path).canonicalize()?;
            if roots
                .iter()
                .any(|root| root.starts_with(&project.path) || project.path.starts_with(root))
            {
                return Err(Error::Config(
                    "project directories must be distinct and must not contain each other".into(),
                ));
            }
            if !project.path.join(".editor/repository.json").is_file()
                && !project.path.join(".editor/schema.json").is_file()
            {
                return Err(Error::Config(format!(
                    "{} is not an editor project",
                    project.path.display()
                )));
            }
            roots.push(project.path.clone());
        }
        if !ids.contains(&catalog.default) {
            return Err(Error::Config("default project is not registered".into()));
        }
        Ok(catalog)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn resolves_relative_paths_and_rejects_ambiguous_registrations() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["one", "two", "one/nested"] {
            fs::create_dir_all(dir.path().join(name).join(".editor")).unwrap();
            fs::write(dir.path().join(name).join(".editor/repository.json"), "{}").unwrap();
        }
        let path = dir.path().join("projects.json");
        let valid = json!({"default":"one","projects":[{"id":"one","name":"First","path":"one"},{"id":"two","name":"Second","path":"two"}]});
        fs::write(&path, valid.to_string()).unwrap();
        let catalog = Projects::load(&path).unwrap();
        assert_eq!(catalog.projects[0].path, dir.path().join("one"));
        for (pointer, value) in [
            ("/default", "missing"),
            ("/projects/1/id", "one"),
            ("/projects/1/id", "../two"),
            ("/projects/1/path", "one"),
            ("/projects/1/path", "one/nested"),
            ("/projects/1/name", " "),
        ] {
            let mut invalid = valid.clone();
            *invalid.pointer_mut(pointer).unwrap() = json!(value);
            fs::write(&path, invalid.to_string()).unwrap();
            assert!(Projects::load(&path).is_err(), "{pointer}: {value}");
        }
    }
}
