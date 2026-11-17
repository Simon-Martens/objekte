//! Lossless stored paths shared by catalog and segment metadata archives.

use std::path::{Path, PathBuf};

use rkyv::{Archive, Serialize};

use crate::inventory::SharedPath;

#[derive(Archive, Serialize)]
pub(crate) enum StoredPath {
    Plain(String),
    Escaped {
        query: String,
        #[cfg(unix)]
        native: Vec<u8>,
        #[cfg(windows)]
        native: Vec<u16>,
    },
}
impl StoredPath {
    pub(crate) fn from_shared(path: &SharedPath) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            if let Some(native) = path.as_path().to_str()
                && path.query_key().as_ref() == native
            {
                return Self::Plain(native.to_owned());
            }
            Self::Escaped {
                query: path.query_key().to_string(),
                native: path.as_path().as_os_str().as_bytes().to_vec(),
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            if let Some(native) = path.as_path().to_str()
                && path.query_key().as_ref() == native
            {
                return Self::Plain(native.to_owned());
            }
            Self::Escaped {
                query: path.query_key().to_string(),
                native: path.as_path().as_os_str().encode_wide().collect(),
            }
        }
    }

    pub(crate) fn from_archived(path: &rkyv::Archived<Self>) -> Self {
        match path {
            rkyv::Archived::<Self>::Plain(value) => Self::Plain(value.as_str().into()),
            rkyv::Archived::<Self>::Escaped { query, native } => {
                #[cfg(unix)]
                {
                    Self::Escaped {
                        query: query.as_str().into(),
                        native: native.as_slice().to_vec(),
                    }
                }
                #[cfg(windows)]
                {
                    Self::Escaped {
                        query: query.as_str().into(),
                        native: native.iter().map(|unit| unit.to_native()).collect(),
                    }
                }
            }
        }
    }

    pub(crate) fn to_shared(&self) -> Result<SharedPath, String> {
        let native = match self {
            Self::Plain(value) => PathBuf::from(value),
            Self::Escaped { native, .. } => {
                #[cfg(unix)]
                {
                    use std::ffi::OsString;
                    use std::os::unix::ffi::OsStringExt;
                    PathBuf::from(OsString::from_vec(native.clone()))
                }
                #[cfg(windows)]
                {
                    use std::ffi::OsString;
                    use std::os::windows::ffi::OsStringExt;
                    PathBuf::from(OsString::from_wide(native))
                }
            }
        };
        let shared = SharedPath::from(native);
        let expected = match self {
            Self::Plain(value) => value.as_str(),
            Self::Escaped { query, .. } => query.as_str(),
        };
        if shared.query_key().as_ref() != expected {
            return Err("stored path does not round-trip losslessly".into());
        }
        Ok(shared)
    }

    pub(crate) fn is_valid_relative(&self) -> bool {
        let path = match self {
            Self::Plain(value) => Path::new(value),
            #[cfg(unix)]
            Self::Escaped { native, .. } => {
                use std::ffi::OsStr;
                use std::os::unix::ffi::OsStrExt;
                Path::new(OsStr::from_bytes(native))
            }
            #[cfg(windows)]
            Self::Escaped { native, .. } => {
                use std::ffi::OsStr;
                use std::os::windows::ffi::OsStrExt;
                Path::new(OsStr::from_wide(native))
            }
        };
        !path.as_os_str().is_empty()
            && !path.is_absolute()
            && path
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
    }
}
pub(crate) fn query(path: &rkyv::Archived<StoredPath>) -> &str {
    match path {
        rkyv::Archived::<StoredPath>::Plain(value) => value.as_str(),
        rkyv::Archived::<StoredPath>::Escaped { query, .. } => query.as_str(),
    }
}

pub(crate) fn to_shared(path: &rkyv::Archived<StoredPath>) -> Result<SharedPath, String> {
    let native = match path {
        rkyv::Archived::<StoredPath>::Plain(value) => PathBuf::from(value.as_str()),
        rkyv::Archived::<StoredPath>::Escaped { native, .. } => {
            #[cfg(unix)]
            {
                use std::ffi::OsString;
                use std::os::unix::ffi::OsStringExt;
                PathBuf::from(OsString::from_vec(native.as_slice().to_vec()))
            }
            #[cfg(windows)]
            {
                use std::ffi::OsString;
                use std::os::windows::ffi::OsStringExt;
                let units = native
                    .iter()
                    .map(|unit| unit.to_native())
                    .collect::<Vec<_>>();
                PathBuf::from(OsString::from_wide(&units))
            }
        }
    };
    let shared = SharedPath::from(native);
    if shared.query_key().as_ref() != query(path) {
        return Err("archived path does not round-trip losslessly".into());
    }
    Ok(shared)
}

pub(crate) fn matches_native(path: &rkyv::Archived<StoredPath>, native: &Path) -> bool {
    match path {
        rkyv::Archived::<StoredPath>::Plain(value) => native.to_str() == Some(value.as_str()),
        rkyv::Archived::<StoredPath>::Escaped { native: stored, .. } => {
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStrExt;
                stored.as_slice() == native.as_os_str().as_bytes()
            }
            #[cfg(windows)]
            {
                use std::os::windows::ffi::OsStrExt;
                stored.iter().copied().eq(native.as_os_str().encode_wide())
            }
        }
    }
}
