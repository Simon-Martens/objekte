//! One validated directory per scan; metadata manifests and descriptor-based reads.
use crate::{
    Error, Result,
    error::{io, record},
    repository::validate_id,
};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File, Metadata},
    path::{Path, PathBuf},
    sync::Mutex,
    time::UNIX_EPOCH,
};


// INFO: Source file size, modification time, and identity for freshness checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Stamp {
    pub size: u64,
    pub modified: String,
    /// Unix device/inode/ctime; empty on platforms without this metadata.
    pub identity: String,
}


#[cfg(all(test, unix))]
#[path = "source.test.rs"]
mod tests;


impl Stamp {
    pub fn from_metadata(m: &Metadata, path: &Path) -> Result<Self> {
        if !m.is_file() {
            return Err(record(path, "", "source must be a regular file"));
        }
        if m.len() > i64::MAX as u64 {
            return Err(record(path, "", "file exceeds supported size"));
        }
        Ok(Self {
            size: m.len(),
            modified: timestamp(m, path)?,
            identity: identity(m),
        })
    }

    #[cfg(all(test, target_os = "linux"))]
    fn from_stat(m: &rustix::fs::Stat, path: &Path) -> Result<Self> {
        if rustix::fs::FileType::from_raw_mode(m.st_mode) != rustix::fs::FileType::RegularFile {
            return Err(record(path, "", "source must be a regular file"));
        }
        if m.st_size < 0 {
            return Err(record(path, "", "file exceeds supported size"));
        }
        // Match std::fs::Metadata exactly, including fractional times before the epoch.
        let modified =
            (i128::from(m.st_mtime) * 1_000_000_000 + i128::from(m.st_mtime_nsec)).to_string();
        Ok(Self {
            size: m.st_size as u64,
            modified,
            identity: format!(
                "{}:{}:{}:{}",
                m.st_dev, m.st_ino, m.st_ctime, m.st_ctime_nsec
            ),
        })
    }
}


fn timestamp(m: &Metadata, path: &Path) -> Result<String> {
    Ok(
        match m
            .modified()
            .map_err(|e| io(path, e))?
            .duration_since(UNIX_EPOCH)
        {
            Ok(v) => v.as_nanos().to_string(),
            Err(e) => format!("-{}", e.duration().as_nanos()),
        },
    )
}

fn identity(m: &Metadata) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        format!("{}:{}:{}:{}", m.dev(), m.ino(), m.ctime(), m.ctime_nsec())
    }

    #[cfg(not(unix))]
    {
        let _ = m;
        String::new()
    }
}


// INFO: Pinned source directory and its original identity for safe record reads.
pub(crate) struct Source {
    pub path: PathBuf,
    recursive: bool,
    #[cfg(unix)]
    directory: File,
    initial: (String, String),
    inventory_dirs: Mutex<Vec<(PathBuf, String, String)>>,
}


impl Source {
    #[cfg(test)]
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_scoped(path, false)
    }

    pub fn open_scoped(path: &Path, recursive: bool) -> Result<Self> {
        if fs::canonicalize(path).map_err(|e| io(path, e))? != path {
            return Err(record(
                path,
                "",
                "source directory identity changed; reload repository",
            ));
        }

        #[cfg(unix)]
        let directory = File::from(
            rustix::fs::open(
                path,
                rustix::fs::OFlags::RDONLY
                    | rustix::fs::OFlags::DIRECTORY
                    | rustix::fs::OFlags::NOFOLLOW
                    | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::empty(),
            )
            .map_err(|e| io(path, e.into()))?,
        );
        #[cfg(unix)]
        let m = directory.metadata().map_err(|e| io(path, e))?;
        #[cfg(not(unix))]
        let m = fs::symlink_metadata(path).map_err(|e| io(path, e))?;
        if !m.is_dir() {
            return Err(record(path, "", "source must be a directory"));
        }
        Ok(Self {
            path: path.into(),
            recursive,
            #[cfg(unix)]
            directory,
            initial: (timestamp(&m, path)?, identity(&m)),
            inventory_dirs: Mutex::new(Vec::new()),
        })
    }

    pub fn inventory(&self) -> Result<BTreeMap<String, Stamp>> {
        let mut paths = Vec::new();
        let mut directories = Vec::new();
        self.walk(&self.path, &mut paths, &mut directories)?;
        let stamps = paths
            .into_par_iter()
            .map(|(id, path)| {
                let stamp = Stamp::from_metadata(
                    &fs::symlink_metadata(&path).map_err(|error| io(&path, error))?,
                    &path,
                )?;
                Ok((id, stamp))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut files = BTreeMap::new();
        for (id, stamp) in stamps {
            files.insert(id, stamp);
        }
        *self
            .inventory_dirs
            .lock()
            .expect("inventory directory lock") = directories;
        self.finish()?;
        Ok(files)
    }

    fn walk(
        &self,
        root: &Path,
        files: &mut Vec<(String, PathBuf)>,
        directories: &mut Vec<(PathBuf, String, String)>,
    ) -> Result<()> {
        let mut pending = vec![root.to_owned()];
        while let Some(directory) = pending.pop() {
            let metadata =
                fs::symlink_metadata(&directory).map_err(|error| io(&directory, error))?;
            if !metadata.is_dir() {
                return Err(record(
                    &directory,
                    "",
                    "source directory must be a real directory",
                ));
            }
            directories.push((
                directory.clone(),
                timestamp(&metadata, &directory)?,
                identity(&metadata),
            ));
            for entry in fs::read_dir(&directory).map_err(|error| io(&directory, error))? {
                let entry = entry.map_err(|error| io(&directory, error))?;
                let path = entry.path();
                let kind = entry.file_type().map_err(|error| io(&path, error))?;
                if kind.is_dir() {
                    if matches!(entry.file_name().to_str(), Some(".editor" | ".git")) {
                        continue;
                    }
                    if self.recursive {
                        pending.push(path);
                    }
                    continue;
                }
                if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                    continue;
                }
                if !kind.is_file() {
                    return Err(record(&path, "", "source JSON must be a regular file"));
                }
                let relative = path
                    .strip_prefix(&self.path)
                    .expect("walk stays inside source")
                    .with_extension("");
                let id = relative
                    .to_str()
                    .ok_or_else(|| record(&path, "", "filename must be UTF-8"))?
                    .replace(std::path::MAIN_SEPARATOR, "/");
                validate_id(&id)?;
                files.push((id, path));
            }
        }
        Ok(())
    }

    pub fn file(&self, id: &str) -> Result<File> {
        validate_id(id)?;
        if !self.recursive && id.contains('/') {
            return Err(record(
                &self.path,
                id,
                "record belongs to another collection",
            ));
        }
        let name = format!("{id}.json");
        let path = self.path.join(&name);
        #[cfg(unix)]
        {
            // Open every parent relative to the pinned base. O_NOFOLLOW on
            // the final file alone would still follow an intermediate link.
            let mut parent: Option<File> = None;
            for component in id.split('/').take(id.split('/').count() - 1) {
                let base = parent.as_ref().unwrap_or(&self.directory);
                parent = Some(File::from(
                    rustix::fs::openat(
                        base,
                        component,
                        rustix::fs::OFlags::RDONLY
                            | rustix::fs::OFlags::DIRECTORY
                            | rustix::fs::OFlags::NOFOLLOW
                            | rustix::fs::OFlags::CLOEXEC,
                        rustix::fs::Mode::empty(),
                    )
                    .map_err(|error| io(&path, error.into()))?,
                ));
            }
            rustix::fs::openat(
                parent.as_ref().unwrap_or(&self.directory),
                format!("{}.json", id.rsplit('/').next().unwrap()).as_str(),
                rustix::fs::OFlags::RDONLY
                    | rustix::fs::OFlags::NOFOLLOW
                    | rustix::fs::OFlags::CLOEXEC
                    | rustix::fs::OFlags::NONBLOCK,
                rustix::fs::Mode::empty(),
            )
            .map(File::from)
            .map_err(|e| io(&path, e.into()))
        }

        #[cfg(not(unix))]
        {
            let mut current = self.path.clone();
            for component in id.split('/').take(id.split('/').count() - 1) {
                current.push(component);
                if !fs::symlink_metadata(&current)
                    .map_err(|error| io(&current, error))?
                    .is_dir()
                {
                    return Err(record(
                        &current,
                        "",
                        "source parent must be a real directory",
                    ));
                }
            }
            Stamp::from_metadata(
                &fs::symlink_metadata(&path).map_err(|e| io(&path, e))?,
                &path,
            )?;
            File::open(&path).map_err(|e| io(&path, e))
        }
    }

    /// Detect directory replacement and membership changes. This is not an atomic FS snapshot.
    pub fn finish(&self) -> Result<()> {
        let m = fs::symlink_metadata(&self.path).map_err(|e| io(&self.path, e))?;
        if !m.is_dir() || (timestamp(&m, &self.path)?, identity(&m)) != self.initial {
            return Err(Error::Unstable(self.path.clone()));
        }
        for (path, modified, id) in self
            .inventory_dirs
            .lock()
            .expect("inventory directory lock")
            .iter()
        {
            let metadata = fs::symlink_metadata(path).map_err(|error| io(path, error))?;
            if !metadata.is_dir()
                || timestamp(&metadata, path)? != *modified
                || identity(&metadata) != *id
            {
                return Err(Error::Unstable(path.clone()));
            }
        }
        Ok(())
    }
}
