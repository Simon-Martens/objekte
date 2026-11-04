//! Descriptor discovery and logical decoding for registered source formats.

use super::*;
use crate::extensions::{
    DecodedRecord, FormatDecoder, ScalarValue, SourceDescriptor, SourceProvider, TypedValue,
};
use crate::source::FilesystemJsonProvider;


/// A logical scalar and its JSON-pointer path within one decoded record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedLeaf {
    pub path: String,
    pub value: TypedValue,
}


pub(super) fn select_sources(
    c: &Collection,
    collection: &str,
    extensions: Option<&crate::extensions::Extensions>,
) -> Result<Vec<SourceDescriptor>> {
    let mut by_record = BTreeMap::new();
    let mut source_ids = BTreeSet::new();
    let filesystem = FilesystemJsonProvider::new(&c.path, c.recursive, collection);
    for descriptor in filesystem.enumerate(collection)? {
        by_record.insert(descriptor.record_id.clone(), descriptor);
    }
    if let Some(extensions) = extensions {
        for provider in extensions.source_providers() {
            if !provider.may_select(collection) {
                continue;
            }
            for descriptor in provider.enumerate(collection)? {
                if descriptor.collection != collection {
                    return Err(Error::Request(format!(
                        "source {} belongs to collection {}, expected {}",
                        descriptor.source_id, descriptor.collection, collection
                    )));
                }
                if descriptor.provider != provider.descriptor().identity {
                    return Err(Error::Request(format!(
                        "source {} declares provider {}, expected {}",
                        descriptor.source_id,
                        descriptor.provider,
                        provider.descriptor().identity
                    )));
                }
                validate_id(&descriptor.record_id)?;
                if descriptor.source_id.is_empty() || descriptor.revision.is_empty() {
                    return Err(Error::Request(format!(
                        "source {} has no stable identity or revision",
                        descriptor.record_id
                    )));
                }
                if !source_ids.insert((descriptor.provider.clone(), descriptor.source_id.clone())) {
                    return Err(Error::Request(format!(
                        "duplicate source identity {}:{}",
                        descriptor.provider, descriptor.source_id
                    )));
                }
                extensions.decoder(&descriptor)?;
                if let Some(previous) =
                    by_record.insert(descriptor.record_id.clone(), descriptor.clone())
                {
                    return Err(Error::Request(format!(
                        "record ID collision for {} between {} and {}",
                        descriptor.record_id,
                        previous.locator.display(),
                        descriptor.locator.display()
                    )));
                }
            }
        }
    }
    Ok(by_record.into_values().collect())
}


impl Repository {
    /// Refresh the descriptor table only when a provider, frontend selection,
    /// or representation posting is active for this collection.
    pub(super) fn reconcile_extension_selection(
        &mut self,
        collection: &str,
    ) -> Result<Option<bool>> {
        let provider_applies = self.extensions.as_ref().is_some_and(|extensions| {
            extensions
                .source_providers()
                .any(|provider| provider.may_select(collection))
        });
        let tracks_selection = provider_applies
            || self.custom_source_snapshot.contains_key(collection)
            || self
                .extension_text_indexes
                .keys()
                .any(|key| key.collection == collection);
        if !tracks_selection {
            return Ok(None);
        }
        let selected = self.source_descriptors(collection)?;
        self.metrics.enumerations += 1;
        let changed = self
            .custom_source_snapshot
            .get(collection)
            .is_none_or(|previous| previous.sources.as_ref() != &selected);
        let has_custom = selected
            .iter()
            .any(|source| source.provider != crate::source::FILESYSTEM_JSON_IDENTITY);
        if changed {
            self.custom_source_snapshot
                .insert(collection.to_owned(), CustomSourceSnapshot::new(selected));
            self.query_cache.invalidate(collection);
            self.invalidate_representation_indexes(collection);
        }
        if has_custom {
            self.custom_source_collections.insert(collection.to_owned());
        } else {
            self.custom_source_collections.remove(collection);
        }
        Ok(Some(has_custom))
    }

    /// Enumerate the built-in JSON sources and registered provider sources in
    /// one collection. Duplicate record IDs fail before any result is exposed.
    pub fn source_descriptors(&self, collection: &str) -> Result<Vec<SourceDescriptor>> {
        let c = self.model.collection(collection)?.clone();
        select_sources(&c, collection, self.extensions.as_deref())
    }

    fn read_source_bytes(&self, descriptor: &SourceDescriptor) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        self.read_source_bytes_checked_into(descriptor, true, &mut bytes)?;
        Ok(bytes)
    }

    /// A query has already validated its collection inventory. Provider reads
    /// still check the selected revision; avoid re-enumerating every source
    /// before and after each record in a scan.
    pub(super) fn read_selected_source_bytes(
        &self,
        descriptor: &SourceDescriptor,
    ) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        self.read_selected_source_bytes_into(descriptor, &mut bytes)?;
        Ok(bytes)
    }

    pub(super) fn read_selected_source_bytes_into(
        &self,
        descriptor: &SourceDescriptor,
        bytes: &mut Vec<u8>,
    ) -> Result<()> {
        self.read_source_bytes_checked_into(descriptor, false, bytes)
    }

    fn read_source_bytes_checked_into(
        &self,
        descriptor: &SourceDescriptor,
        check_inventory: bool,
        bytes: &mut Vec<u8>,
    ) -> Result<()> {
        let collection = descriptor.collection.as_str();
        if check_inventory && !self.source_descriptors(collection)?.contains(descriptor) {
            return Err(Error::Request(
                "source descriptor is stale or unknown".into(),
            ));
        }
        bytes.clear();
        if descriptor.provider == crate::source::FILESYSTEM_JSON_IDENTITY {
            let c = self.model.collection(collection)?.clone();
            FilesystemJsonProvider::new(&c.path, c.recursive, collection)
                .read(descriptor, bytes)?;
        } else {
            let extensions = self
                .extensions
                .as_ref()
                .ok_or_else(|| Error::Request("source provider is no longer registered".into()))?;
            let provider = extensions
                .source_providers()
                .find(|provider| provider.descriptor().identity == descriptor.provider)
                .ok_or_else(|| Error::Request("source provider is no longer registered".into()))?;
            provider.read(descriptor, bytes)?;
        }
        if bytes.len() > i64::MAX as usize {
            return Err(Error::Request("source exceeds supported size".into()));
        }
        if check_inventory && !self.source_descriptors(collection)?.contains(descriptor) {
            return Err(Error::Unstable(descriptor.locator.clone()));
        }
        Ok(())
    }

    /// Read one source and lend its decoded record only during `visit`. The
    /// descriptor is rechecked before and after the provider read. No owned
    /// JSON tree or leaf list is required by this API.
    pub fn with_source_record<T>(
        &self,
        descriptor: &SourceDescriptor,
        visit: impl FnOnce(&dyn DecodedRecord) -> Result<T>,
    ) -> Result<T> {
        let bytes = self.read_source_bytes(descriptor)?;
        let mut visit = Some(visit);
        let mut result = None;
        let mut callback = |record: &dyn DecodedRecord| {
            let visit = visit
                .take()
                .ok_or_else(|| Error::Request("format decoder emitted multiple records".into()))?;
            result = Some(visit(record)?);
            Ok(())
        };
        if let Some(registry) = &self.extensions {
            registry
                .decoder(descriptor)?
                .decode(&bytes, &mut callback)?;
        } else {
            crate::extensions::json_decoder().decode(&bytes, &mut callback)?;
        }
        result.ok_or_else(|| Error::Request("format decoder emitted no record".into()))
    }

    /// Convenience inspection API. Output is published only after successful
    /// complete decoding; callback failures cannot expose partial leaves.
    pub fn read_source_leaves(&self, descriptor: &SourceDescriptor) -> Result<Vec<DecodedLeaf>> {
        self.with_source_record(descriptor, |record| {
            let mut leaves = Vec::new();
            record.visit_leaves(&mut |path, scalar| {
                let value = match scalar {
                    ScalarValue::Null => TypedValue::Null,
                    ScalarValue::Bool(value) => TypedValue::Bool(value),
                    ScalarValue::Number(value) => TypedValue::Number(value.into()),
                    ScalarValue::Text(value) => TypedValue::Text(value.into()),
                };
                leaves.push(DecodedLeaf {
                    path: path.into(),
                    value,
                });
                Ok(())
            })?;
            Ok(leaves)
        })
    }
}
