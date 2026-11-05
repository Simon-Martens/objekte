//! Optional, on-demand Git status. It is never part of the query hot path.

use crate::{Error, Result};
use std::{collections::BTreeMap, path::Path, process::Command};


/// Git index and worktree status of one record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GitState {
    pub index: char,
    pub worktree: char,
}


pub(super) fn status(root: &Path) -> Result<Option<BTreeMap<String, GitState>>> {
    let prefix = match Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--show-prefix"])
        .output()
    {
        Ok(output) if output.status.success() => String::from_utf8(output.stdout)
            .map_err(|e| Error::Cache(format!("Git prefix is not UTF-8: {e}")))?,
        _ => return Ok(None),
    };
    let prefix = prefix.trim_end_matches(['\n', '\r']);
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "status",
            "--porcelain=v1",
            "-z",
            "--no-renames",
            "--untracked-files=all",
            "--",
            ".",
        ])
        .output()
        .map_err(|e| Error::Cache(format!("cannot run git status: {e}")))?;
    if !output.status.success() {
        return Err(Error::Cache(format!(
            "git status failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let mut states = BTreeMap::new();
    for record in output
        .stdout
        .split(|&byte| byte == 0)
        .filter(|part| !part.is_empty())
    {
        if record.len() < 4 || record[2] != b' ' {
            return Err(Error::Cache("malformed Git porcelain status".into()));
        }
        let path = std::str::from_utf8(&record[3..])
            .map_err(|e| Error::Cache(format!("Git path is not UTF-8: {e}")))?;
        let Some(path) = path.strip_prefix(prefix) else {
            continue;
        };
        if !path.ends_with(".json") {
            continue;
        }
        let id = &path[..path.len() - ".json".len()];
        if crate::repository::validate_id(id).is_ok() {
            states.insert(
                id.to_owned(),
                GitState {
                    index: record[0] as char,
                    worktree: record[1] as char,
                },
            );
        }
    }
    Ok(Some(states))
}
