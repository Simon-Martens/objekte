//! Lossless stored paths shared by catalog and segment metadata archives.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rkyv::{Archive, Serialize};
use rkyv::{
    Place,
    rancor::Fallible,
    ser::{Allocator, Writer},
    string::{ArchivedString, StringResolver},
    vec::{ArchivedVec, VecResolver},
};

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

#[cfg(unix)]
type ArchivedNativeUnit = u8;
#[cfg(windows)]
type ArchivedNativeUnit = rkyv::primitive::ArchivedU16;

pub(crate) enum BorrowedPathString<'a> {
    Borrowed(&'a str),
    Shared(Arc<str>),
}

impl BorrowedPathString<'_> {
    fn as_str(&self) -> &str {
        match self {
            Self::Borrowed(value) => value,
            Self::Shared(value) => value,
        }
    }
}

impl Archive for BorrowedPathString<'_> {
    type Archived = ArchivedString;
    type Resolver = StringResolver;

    fn resolve(&self, resolver: Self::Resolver, out: Place<Self::Archived>) {
        ArchivedString::resolve_from_str(self.as_str(), resolver, out);
    }
}

impl<S> Serialize<S> for BorrowedPathString<'_>
where
    S: Fallible + Writer + ?Sized,
    S::Error: rkyv::rancor::Source,
{
    fn serialize(&self, serializer: &mut S) -> Result<<Self as Archive>::Resolver, S::Error> {
        ArchivedString::serialize_from_str(self.as_str(), serializer)
    }
}

pub(crate) struct BorrowedPathNative<'a>(&'a Path);

impl Archive for BorrowedPathNative<'_> {
    type Archived = ArchivedVec<ArchivedNativeUnit>;
    type Resolver = VecResolver;

    fn resolve(&self, resolver: Self::Resolver, out: Place<Self::Archived>) {
        #[cfg(unix)]
        let len = {
            use std::os::unix::ffi::OsStrExt;
            self.0.as_os_str().as_bytes().len()
        };
        #[cfg(windows)]
        let len = {
            use std::os::windows::ffi::OsStrExt;
            self.0.as_os_str().encode_wide().count()
        };
        ArchivedVec::<ArchivedNativeUnit>::resolve_from_len(len, resolver, out);
    }
}

impl<S> Serialize<S> for BorrowedPathNative<'_>
where
    S: Fallible + Allocator + Writer + ?Sized,
    S::Error: rkyv::rancor::Source,
{
    fn serialize(&self, serializer: &mut S) -> Result<<Self as Archive>::Resolver, S::Error> {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            ArchivedVec::<ArchivedNativeUnit>::serialize_from_slice(
                self.0.as_os_str().as_bytes(),
                serializer,
            )
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            ArchivedVec::<ArchivedNativeUnit>::serialize_from_iter(
                self.0.as_os_str().encode_wide(),
                serializer,
            )
        }
    }
}

#[derive(Archive, Serialize)]
#[rkyv(as = rkyv::Archived<StoredPath>)]
pub(crate) enum BorrowedStoredPath<'a> {
    Plain(BorrowedPathString<'a>),
    Escaped {
        query: BorrowedPathString<'a>,
        native: BorrowedPathNative<'a>,
    },
}

impl<'a> BorrowedStoredPath<'a> {
    pub(crate) fn from_shared(path: &'a SharedPath) -> Self {
        let native_text = path.as_path().to_str();
        let mut query = native_text.map(|_| path.query_key());
        if let (Some(native), Some(query)) = (native_text, query.as_deref())
            && query == native
        {
            return Self::Plain(BorrowedPathString::Borrowed(native));
        }
        Self::Escaped {
            query: BorrowedPathString::Shared(query.take().unwrap_or_else(|| path.query_key())),
            native: BorrowedPathNative(path.as_path()),
        }
    }
}

impl StoredPath {
    #[cfg(test)]
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

#[cfg(test)]
#[path = "archived_path.test.rs"]
mod tests;
