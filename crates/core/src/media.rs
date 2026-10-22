use crate::{Editor, Error, Result, safe_name};
use serde::Serialize;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    File,
    Directory,
}

#[derive(Debug, Clone, Serialize)]
pub struct MediaEntry {
    pub name: String,
    pub path: String,
    pub kind: MediaKind,
    pub size: u64,
}

impl Editor {
    pub fn media_search(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<MediaEntry>> {
        let root = self.config().media_dir()?;
        let mut entries = Vec::new();
        collect_media_files(&root, "", &mut entries)?;
        let query = query.to_lowercase();
        entries.retain(|entry| entry.path.to_lowercase().contains(&query));
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(entries
            .into_iter()
            .skip(offset)
            .take(limit.min(100))
            .collect())
    }

    pub fn media_list(&self, dir: &str, offset: usize, limit: usize) -> Result<Vec<MediaEntry>> {
        let path = self.media_path(dir, true)?;
        if !path.is_dir() {
            return Err(Error::InvalidPath(dir.into()));
        }
        let mut entries = Vec::new();
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            if entry.file_type()?.is_symlink() {
                continue;
            }
            let meta = entry.metadata()?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name == ".gitkeep" {
                continue;
            }
            let relative = if dir.is_empty() {
                name.clone()
            } else {
                format!("{dir}/{name}")
            };
            entries.push(MediaEntry {
                name,
                path: relative,
                kind: if meta.is_dir() {
                    MediaKind::Directory
                } else {
                    MediaKind::File
                },
                size: meta.len(),
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries
            .into_iter()
            .skip(offset)
            .take(limit.min(100))
            .collect())
    }

    pub fn media_read(&self, path: &str) -> Result<Vec<u8>> {
        let path = self.media_path(path, true)?;
        if !path.is_file() {
            return Err(Error::NotFound(path.display().to_string()));
        }
        Ok(fs::read(path)?)
    }

    pub fn media_mkdir(&self, path: &str) -> Result<()> {
        let path = self.media_path(path, false)?;
        fs::create_dir(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                Error::Conflict("media path already exists".into())
            } else {
                Error::Io(e)
            }
        })?;
        Ok(())
    }

    pub fn media_upload(&self, path: &str, bytes: &[u8], max_bytes: usize) -> Result<()> {
        if bytes.len() > max_bytes {
            return Err(Error::Data("upload exceeds configured limit".into()));
        }
        let path = self.media_path(path, false)?;
        let parent = path
            .parent()
            .ok_or_else(|| Error::InvalidPath(path.display().to_string()))?;
        let temp = parent.join(format!(".editor-upload-{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| -> Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            fs::hard_link(&temp, &path).map_err(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    Error::Conflict("media path already exists".into())
                } else {
                    Error::Io(e)
                }
            })?;
            fs::remove_file(&temp)?;
            fs::File::open(parent)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result
    }

    pub fn media_move(&self, source: &str, target: &str) -> Result<()> {
        let _write_guard = self.write_guard.lock().unwrap();
        let source_path = self.media_path(source, true)?;
        let target_path = self.media_path(target, false)?;
        if source.is_empty() {
            return Err(Error::InvalidPath("cannot move media root".into()));
        }
        if !self.media_references_under(source)?.is_empty() {
            return Err(Error::Conflict(
                "media path is referenced by a record".into(),
            ));
        }
        if target_path.exists() {
            return Err(Error::Conflict("destination already exists".into()));
        }
        if target_path.starts_with(&source_path) {
            return Err(Error::InvalidPath(
                "cannot move directory into itself".into(),
            ));
        }
        fs::rename(source_path, target_path)?;
        Ok(())
    }

    pub fn media_delete(&self, path: &str) -> Result<()> {
        let _write_guard = self.write_guard.lock().unwrap();
        let target_path = self.media_path(path, true)?;
        if target_path == self.config().media_dir()? {
            return Err(Error::InvalidPath("cannot delete media root".into()));
        }
        if !self.media_references_under(path)?.is_empty() {
            return Err(Error::Conflict(
                "media path is referenced by a record".into(),
            ));
        }
        if target_path.is_dir() {
            fs::remove_dir(target_path)?;
        } else {
            fs::remove_file(target_path)?;
        }
        Ok(())
    }

    pub(crate) fn media_path(&self, relative: &str, must_exist: bool) -> Result<PathBuf> {
        let root = self.config().media_dir()?;
        let mut path = root.clone();
        if relative.is_empty() {
            return if must_exist {
                Ok(root)
            } else {
                Err(Error::InvalidPath("empty media path".into()))
            };
        }
        for component in Path::new(relative).components() {
            let std::path::Component::Normal(name) = component else {
                return Err(Error::InvalidPath(relative.into()));
            };
            let name = name
                .to_str()
                .ok_or_else(|| Error::InvalidPath(relative.into()))?;
            safe_name(name)?;
            path.push(name);
            if path.exists() && fs::symlink_metadata(&path)?.file_type().is_symlink() {
                return Err(Error::InvalidPath(relative.into()));
            }
        }
        let parent = path
            .parent()
            .ok_or_else(|| Error::InvalidPath(relative.into()))?;
        if !parent.canonicalize()?.starts_with(&root) {
            return Err(Error::InvalidPath(relative.into()));
        }
        if must_exist && !path.exists() {
            return Err(Error::NotFound(relative.into()));
        }
        Ok(path)
    }
}

fn collect_media_files(dir: &Path, relative: &str, entries: &mut Vec<MediaEntry>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() || (!kind.is_file() && !kind.is_dir()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name == ".gitkeep" {
            continue;
        }
        let path = if relative.is_empty() {
            name.clone()
        } else {
            format!("{relative}/{name}")
        };
        if kind.is_dir() {
            collect_media_files(&entry.path(), &path, entries)?;
        } else {
            entries.push(MediaEntry {
                name,
                path,
                kind: MediaKind::File,
                size: entry.metadata()?.len(),
            });
        }
    }
    Ok(())
}
