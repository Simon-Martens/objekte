//! One validated directory per scan; metadata manifests and descriptor-based reads.
use crate::{
    Error, Result,
    error::{io, record},
    repository::validate_id,
};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    fs::{self, File, Metadata},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Stamp {
    pub size: u64,
    pub modified: String,
    /// Unix device/inode/ctime; empty on platforms without this metadata.
    pub identity: String,
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{io::Read, os::unix::fs::symlink};
    #[test]
    fn pinned_directory_survives_replacement_but_finish_rejects_it() {
        let temp = tempfile::tempdir().unwrap();
        let p = temp.path().join("source");
        fs::create_dir(&p).unwrap();
        fs::write(p.join("a.json"), "old").unwrap();
        let source = Source::open(&p).unwrap();
        fs::rename(&p, temp.path().join("old-source")).unwrap();
        fs::create_dir(&p).unwrap();
        fs::write(p.join("a.json"), "new").unwrap();
        let mut text = String::new();
        source.file("a").unwrap().read_to_string(&mut text).unwrap();
        assert_eq!(text, "old");
        assert!(source.finish().is_err());
    }
    #[test]
    fn symlink_swapped_in_after_inventory_is_never_followed() {
        let temp = tempfile::tempdir().unwrap();
        let p = temp.path().join("source");
        fs::create_dir(&p).unwrap();
        fs::write(p.join("a.json"), "original").unwrap();
        let source = Source::open(&p).unwrap();
        source.inventory().unwrap();
        let external = temp.path().join("external.json");
        fs::write(&external, "external").unwrap();
        fs::remove_file(p.join("a.json")).unwrap();
        symlink(&external, p.join("a.json")).unwrap();
        assert!(source.file("a").is_err());
        assert!(source.finish().is_err());
    }
    #[test]
    fn membership_changes_after_inventory_are_detected() {
        let temp = tempfile::tempdir().unwrap();
        let source = Source::open(temp.path()).unwrap();
        source.inventory().unwrap();
        fs::write(temp.path().join("new.json"), "{}").unwrap();
        assert!(source.finish().is_err());
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn relative_stat_preserves_manifest_stamps_including_pre_epoch_timestamps() {
        let temp = tempfile::tempdir().unwrap();
        let p = temp.path().join("unicode-é.json");
        fs::write(&p, "{}").unwrap();
        for modified in [
            UNIX_EPOCH + std::time::Duration::new(1234, 567890123),
            UNIX_EPOCH - std::time::Duration::new(1, 234567890),
        ] {
            File::options()
                .write(true)
                .open(&p)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(modified))
                .unwrap();
            let source = Source::open(temp.path()).unwrap();
            let inventory = source.inventory().unwrap();
            assert_eq!(
                inventory["unicode-é"],
                Stamp::from_metadata(&fs::symlink_metadata(&p).unwrap(), &p).unwrap()
            );
        }
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn relative_stat_rejects_a_symlink_even_when_its_target_is_regular() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("real.json"), "{}").unwrap();
        symlink("real.json", temp.path().join("link.json")).unwrap();
        let source = Source::open(temp.path()).unwrap();
        let stat = rustix::fs::statat(
            &source.directory,
            "link.json",
            rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
        )
        .unwrap();
        assert!(Stamp::from_stat(&stat, &temp.path().join("link.json")).is_err());
    }
}
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
        let mut files = BTreeMap::new();
        for entry in fs::read_dir(&self.path).map_err(|e| io(&self.path, e))? {
            let entry = entry.map_err(|e| io(&self.path, e))?;
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
                continue;
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
            files.insert(id.into(), stamp);
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
    /// Detect directory replacement and membership changes. This is not an atomic FS snapshot.
    pub fn finish(&self) -> Result<()> {
        let m = fs::symlink_metadata(&self.path).map_err(|e| io(&self.path, e))?;
        if !m.is_dir() || (timestamp(&m, &self.path)?, identity(&m)) != self.initial {
            return Err(Error::Unstable(self.path.clone()));
        }
        Ok(())
    }
}
