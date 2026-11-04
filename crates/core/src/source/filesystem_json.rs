//! Built-in JSON source adapter. The picker keeps its concrete fast path;
//! registered frontends use the same source contract as external providers.

use super::{Source, Stamp};
use crate::{
    Error, Result,
    config::fingerprint,
    error::io,
    extensions::{
        DataKind, Dependency, Descriptor, SemanticVersion, SourceDescriptor, SourceProvider,
    },
};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    sync::LazyLock,
};

pub(crate) const IDENTITY: &str = "objekte:filesystem";


pub(crate) struct FilesystemJsonProvider<'a> {
    descriptor: &'static Descriptor,
    root: &'a Path,
    recursive: bool,
    collection: &'a str,
}


impl<'a> FilesystemJsonProvider<'a> {
    pub(crate) fn new(root: &'a Path, recursive: bool, collection: &'a str) -> Self {
        static FLAT: LazyLock<Descriptor> = LazyLock::new(|| descriptor(false));
        static RECURSIVE: LazyLock<Descriptor> = LazyLock::new(|| descriptor(true));
        Self {
            descriptor: if recursive { &RECURSIVE } else { &FLAT },
            root,
            recursive,
            collection,
        }
    }
}


fn descriptor(recursive: bool) -> Descriptor {
    Descriptor::new(
        IDENTITY,
        SemanticVersion {
            major: 1,
            minor: 0,
            patch: 0,
        },
        &(recursive, "json"),
        DataKind::Source,
        DataKind::Source,
        vec![Dependency::FileName, Dependency::SourceBytes],
    )
}


impl SourceProvider for FilesystemJsonProvider<'_> {
    fn descriptor(&self) -> &Descriptor {
        self.descriptor
    }

    fn may_select(&self, collection: &str) -> bool {
        collection == self.collection
    }

    fn enumerate(&self, collection: &str) -> Result<Vec<SourceDescriptor>> {
        if !self.may_select(collection) {
            return Ok(Vec::new());
        }
        let source = Source::open_scoped(self.root, self.recursive)?;
        let mut descriptors = Vec::new();
        for (id, stamp) in source.inventory()? {
            let relative = PathBuf::from(format!("{id}.json"));
            descriptors.push(SourceDescriptor {
                collection: collection.into(),
                provider: IDENTITY.into(),
                source_id: relative.to_string_lossy().into_owned(),
                record_id: id,
                locator: relative,
                format: "objekte:json".into(),
                revision: fingerprint("source-stamp/v1", &stamp),
                metadata: BTreeMap::from([
                    ("size_bytes".into(), stamp.size.to_string()),
                    ("modified_unix_nanos".into(), stamp.modified),
                ]),
            });
        }
        source.finish()?;
        Ok(descriptors)
    }

    fn read(&self, descriptor: &SourceDescriptor, output: &mut Vec<u8>) -> Result<()> {
        if descriptor.collection != self.collection
            || descriptor.provider != IDENTITY
            || descriptor.format != "objekte:json"
            || descriptor
                .locator
                .to_str()
                .and_then(|path| path.strip_suffix(".json"))
                != Some(descriptor.record_id.as_str())
            || descriptor.locator.to_str() != Some(descriptor.source_id.as_str())
        {
            return Err(Error::Request(
                "invalid built-in JSON source descriptor".into(),
            ));
        }
        let source = Source::open_scoped(self.root, self.recursive)?;
        let path = self.root.join(&descriptor.locator);
        let mut file = source.file_relative(&descriptor.locator)?;
        let before =
            Stamp::from_metadata(&file.metadata().map_err(|error| io(&path, error))?, &path)?;
        if fingerprint("source-stamp/v1", &before) != descriptor.revision {
            return Err(Error::Unstable(path));
        }
        output.clear();
        file.read_to_end(output).map_err(|error| io(&path, error))?;
        let after =
            Stamp::from_metadata(&file.metadata().map_err(|error| io(&path, error))?, &path)?;
        if before != after || output.len() as u64 != after.size {
            return Err(Error::Unstable(path));
        }
        source.finish()?;
        Ok(())
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn selected_json_uses_real_locator_and_revision_checked_read() {
        let temp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(temp.path()).unwrap();
        fs::create_dir(root.join("nested")).unwrap();
        fs::write(root.join("nested/item.json"), br#"{"body":"first"}"#).unwrap();
        let provider = FilesystemJsonProvider::new(&root, true, "books");
        let descriptors = provider.enumerate("books").unwrap();
        assert_eq!(descriptors.len(), 1);
        let selected = &descriptors[0];
        assert_eq!(selected.record_id, "nested/item");
        assert_eq!(selected.source_id, "nested/item.json");
        assert_eq!(selected.locator, Path::new("nested/item.json"));
        assert_eq!(selected.provider, IDENTITY);
        let mut bytes = Vec::new();
        provider.read(selected, &mut bytes).unwrap();
        assert_eq!(bytes, br#"{"body":"first"}"#);
        fs::write(root.join("nested/item.json"), br#"{"body":"changed"}"#).unwrap();
        assert!(matches!(
            provider.read(selected, &mut bytes),
            Err(Error::Unstable(_))
        ));
    }
}
