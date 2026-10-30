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

    #[cfg(target_os = "linux")]
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
    #[cfg(unix)]
    directory: File,
    initial: (String, String),
}


impl Source {
    pub fn open(path: &Path) -> Result<Self> {
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
            #[cfg(unix)]
            directory,
            initial: (timestamp(&m, path)?, identity(&m)),
        })
    }

    pub fn inventory(&self) -> Result<BTreeMap<String, Stamp>> {
        let entries = fs::read_dir(&self.path)
            .map_err(|e| io(&self.path, e))?
            .collect::<std::io::Result<Vec<_>>>()
            .map_err(|e| io(&self.path, e))?;
        let stamps = entries
            .into_par_iter()
            .map(|entry| -> Result<Option<(String, Stamp)>> {
                let path = entry.path();
                let ty = entry.file_type().map_err(|e| io(&path, e))?;
                if ty.is_dir() {
                    return Err(record(
                        &path,
                        "",
                        "nested source directories are not supported",
                    ));
                }
                if path.extension().and_then(|s| s.to_str()) != Some("json") {
                    return Ok(None);
                }
                if !ty.is_file() {
                    return Err(record(
                        &path,
                        "",
                        "source JSON must be a regular file, not a symlink",
                    ));
                }
                let id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .ok_or_else(|| record(&path, "", "filename must be UTF-8"))?;
                validate_id(id)?;
                #[cfg(target_os = "linux")]
                let stamp = Stamp::from_stat(
                    &rustix::fs::statat(
                        &self.directory,
                        path.file_name().unwrap(),
                        rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
                    )
                    .map_err(|e| io(&path, e.into()))?,
                    &path,
                )?;
                #[cfg(not(target_os = "linux"))]
                let stamp = Stamp::from_metadata(
                    &fs::symlink_metadata(&path).map_err(|e| io(&path, e))?,
                    &path,
                )?;
                Ok(Some((id.into(), stamp)))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut files = BTreeMap::new();
        for (id, stamp) in stamps.into_iter().flatten() {
            files.insert(id, stamp);
        }
        self.finish()?;
        Ok(files)
    }

    pub fn file(&self, id: &str) -> Result<File> {
        validate_id(id)?;
        let name = format!("{id}.json");
        let path = self.path.join(&name);
        #[cfg(unix)]
        {
            // No symlink following or blocking on a raced-in FIFO. Pin reads to this directory.
            rustix::fs::openat(
                &self.directory,
                name.as_str(),
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
            Stamp::from_metadata(
                &fs::symlink_metadata(&path).map_err(|e| io(&path, e))?,
                &path,
            )?;
            File::open(&path).map_err(|e| io(&path, e))
        }
    }

    /// Check an indexed match without reading or parsing its contents.
    pub fn check_stamp(&self, id: &str, expected: &Stamp) -> Result<()> {
        validate_id(id)?;
        let name = format!("{id}.json");
        let path = self.path.join(&name);
        #[cfg(target_os = "linux")]
        let current = Stamp::from_stat(
            &rustix::fs::statat(
                &self.directory,
                name.as_str(),
                rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
            )
            .map_err(|e| io(&path, e.into()))?,
            &path,
        )?;
        #[cfg(not(target_os = "linux"))]
        let current = Stamp::from_metadata(
            &fs::symlink_metadata(&path).map_err(|e| io(&path, e))?,
            &path,
        )?;
        if &current != expected {
            return Err(Error::Unstable(path));
        }
        Ok(())
    }

    /// Detect directory replacement and membership changes. This is not an atomic FS snapshot.
    pub fn finish(&self) -> Result<()> {
        let m = fs::symlink_metadata(&self.path).map_err(|e| io(&self.path, e))?;
        if !m.is_dir() || (timestamp(&m, &self.path)?, identity(&m)) != self.initial {
            return Err(Error::Unstable(self.path.clone()));
        }
        Ok(())
    }
}
