//! The V5 manifest stores IDs once and retains numeric source metadata.

use super::{SnapshotFiles, invalid};
use crate::{Result, file_picker, repository::validate_id, source::Stamp};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{SeqAccess, Visitor},
    ser::SerializeSeq,
};
use std::{borrow::Cow, collections::BTreeMap, fmt, path::PathBuf};


#[derive(Clone, Serialize, Deserialize)]
pub(crate) enum FileIdentity {
    Empty,
    Unix {
        device: u64,
        inode: u64,
        changed_seconds: i64,
        changed_nanos: i64,
    },
    Other(String),
}


fn canonical_decimal(text: &str, signed: bool) -> bool {
    let digits = if signed {
        text.strip_prefix('-').unwrap_or(text)
    } else {
        text
    };
    !digits.is_empty()
        && digits.bytes().all(|byte| byte.is_ascii_digit())
        && (digits == "0" || !digits.starts_with('0'))
        && text != "-0"
}


impl FileIdentity {
    pub(super) fn from_text(text: &str) -> Self {
        if text.is_empty() {
            return Self::Empty;
        }
        let mut parts = text.split(':');
        if let (Some(device_text), Some(inode_text), Some(seconds), Some(nanos), None) = (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) && let (Ok(device), Ok(inode), Ok(changed_seconds), Ok(changed_nanos)) = (
            device_text.parse(),
            inode_text.parse(),
            seconds.parse(),
            nanos.parse(),
        ) && canonical_decimal(device_text, false)
            && canonical_decimal(inode_text, false)
            && canonical_decimal(seconds, true)
            && canonical_decimal(nanos, true)
        {
            return Self::Unix {
                device,
                inode,
                changed_seconds,
                changed_nanos,
            };
        }
        Self::Other(text.to_owned())
    }

    pub(crate) fn text(self) -> String {
        match self {
            Self::Empty => String::new(),
            Self::Unix {
                device,
                inode,
                changed_seconds,
                changed_nanos,
            } => format!("{device}:{inode}:{changed_seconds}:{changed_nanos}"),
            Self::Other(text) => text,
        }
    }
}


#[derive(Serialize, Deserialize)]
pub(crate) struct FileEntry {
    pub id_start: u32,
    pub id_len: u32,
    pub size_bytes: u64,
    pub modified_unix_nanos: i128,
    pub identity: FileIdentity,
}


struct EntriesRef<'a, F: SnapshotFiles + ?Sized>(&'a F);


impl<F: SnapshotFiles + ?Sized> Serialize for EntriesRef<'_, F> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::Error as _;
        let mut output = serializer.serialize_seq(Some(self.0.len()))?;
        let mut start = 0_u32;
        for slot in 0..self.0.len() {
            let file = self.0.entry(slot);
            let len = u32::try_from(file.id.len()).map_err(S::Error::custom)?;
            let entry = FileEntry {
                id_start: start,
                id_len: len,
                size_bytes: file.size_bytes,
                modified_unix_nanos: file.modified_unix_nanos.parse().map_err(S::Error::custom)?,
                identity: FileIdentity::from_text(file.identity),
            };
            start = start
                .checked_add(len)
                .ok_or_else(|| S::Error::custom("ID arena too large"))?;
            output.serialize_element(&entry)?;
        }
        output.end()
    }
}


#[derive(Serialize)]
#[serde(bound(serialize = ""))]
pub(super) struct ManifestRef<'a, F: SnapshotFiles + ?Sized> {
    pub source_root: &'a PathBuf,
    pub recursive: bool,
    pub git_commit: &'a Option<String>,
    pub id_bytes: Cow<'a, [u8]>,
    entries: EntriesRef<'a, F>,
    pub changed_slots: &'a [usize],
    pub changed_bigrams: &'a BTreeMap<usize, Vec<u16>>,
    pub bigram_encoding: &'static str,
    pub scalar_postings: bool,
}


impl<'a, F: SnapshotFiles + ?Sized> ManifestRef<'a, F> {
    #[expect(
        clippy::too_many_arguments,
        reason = "snapshot scope and manifest parts are independent"
    )]
    pub(super) fn new(
        source_root: &'a PathBuf,
        recursive: bool,
        git_commit: &'a Option<String>,
        files: &'a F,
        changed_slots: &'a [usize],
        changed_bigrams: &'a BTreeMap<usize, Vec<u16>>,
        bigram_encoding: &'static str,
        scalar_postings: bool,
    ) -> Self {
        Self {
            source_root,
            recursive,
            git_commit,
            id_bytes: files.id_arena(),
            entries: EntriesRef(files),
            changed_slots,
            changed_bigrams,
            bigram_encoding,
            scalar_postings,
        }
    }

    pub(super) fn files(&self) -> &F {
        self.entries.0
    }
}


#[cfg(test)]
#[derive(Serialize)]
pub(super) struct Manifest {
    pub source_root: PathBuf,
    pub recursive: bool,
    pub git_commit: Option<String>,
    pub id_bytes: Vec<u8>,
    pub entries: Vec<FileEntry>,
    pub changed_slots: Vec<usize>,
    pub changed_bigrams: BTreeMap<usize, Vec<u16>>,
    pub bigram_encoding: String,
    pub scalar_postings: bool,
}


pub(super) struct DirectEntries(pub Vec<Option<file_picker::FileEntry>>);


impl<'de> Deserialize<'de> for DirectEntries {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct EntriesVisitor;


        impl<'de> Visitor<'de> for EntriesVisitor {
            type Value = DirectEntries;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a sequence of compact file entries")
            }

            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut files = Vec::with_capacity(sequence.size_hint().unwrap_or(0).min(65_536));
                let changed_at_nanos = file_picker::now_nanos();
                while let Some(entry) = sequence.next_element::<FileEntry>()? {
                    files.push(Some(file_picker::FileEntry {
                        id_start: entry.id_start,
                        id_len: entry.id_len,
                        stamp: Stamp {
                            size: entry.size_bytes,
                            modified: entry.modified_unix_nanos.to_string(),
                            identity: entry.identity.text(),
                        },
                        revision: 1,
                        changed_at_nanos,
                    }));
                }
                Ok(DirectEntries(files))
            }
        }


        deserializer.deserialize_seq(EntriesVisitor)
    }
}


#[derive(Deserialize)]
pub(super) struct DecodedManifest {
    pub source_root: PathBuf,
    pub recursive: bool,
    pub git_commit: Option<String>,
    pub id_bytes: Vec<u8>,
    pub entries: DirectEntries,
    pub changed_slots: Vec<usize>,
    pub changed_bigrams: BTreeMap<usize, Vec<u16>>,
    pub bigram_encoding: String,
    pub scalar_postings: bool,
}


impl DecodedManifest {
    pub(super) fn validate(&self, file_count: usize) -> Result<()> {
        if self.entries.0.len() != file_count || self.id_bytes.len() > u32::MAX as usize {
            return Err(invalid("file count or ID arena differs from filter slots"));
        }
        let mut previous: Option<&str> = None;
        let mut end = 0_usize;
        for saved in &self.entries.0 {
            let entry = saved
                .as_ref()
                .ok_or_else(|| invalid("missing compact file entry"))?;
            let start = entry.id_start as usize;
            let next = start
                .checked_add(entry.id_len as usize)
                .ok_or_else(|| invalid("ID offset overflow"))?;
            if start != end || next > self.id_bytes.len() || entry.stamp.size > i64::MAX as u64 {
                return Err(invalid("invalid compact file entry"));
            }
            let id = std::str::from_utf8(&self.id_bytes[start..next])
                .map_err(|_| invalid("invalid UTF-8 file ID"))?;
            validate_id(id).map_err(|_| invalid("invalid file ID"))?;
            if previous.is_some_and(|last| last >= id) {
                return Err(invalid("duplicate or unordered file ID"));
            }
            previous = Some(id);
            end = next;
        }
        if end != self.id_bytes.len() {
            return Err(invalid("unreferenced ID bytes"));
        }
        Ok(())
    }
}
