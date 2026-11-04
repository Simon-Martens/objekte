//! Descriptor discovery and logical decoding for registered source formats.

use super::*;
use crate::extensions::{
    FormatDecoder, JsonFormatDecoder, ScalarValue, SourceDescriptor, TypedValue,
};

const FILESYSTEM_PROVIDER: &str = "objekte:filesystem";


/// A logical scalar and its JSON-pointer path within one decoded record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedLeaf {
    pub path: String,
    pub value: TypedValue,
}


impl Repository {
    /// Enumerate the built-in JSON sources and registered provider sources in
    /// one collection. Duplicate record IDs fail before any result is exposed.
    pub fn source_descriptors(&self, collection: &str) -> Result<Vec<SourceDescriptor>> {
        let c = self.collection(collection)?;
        let source = Source::open_scoped(&c.path, c.recursive)?;
        let mut by_record = BTreeMap::new();
        let mut source_ids = BTreeSet::new();
        for (id, stamp) in source.inventory()? {
            let relative = PathBuf::from(format!("{id}.json"));
            let descriptor = SourceDescriptor {
                collection: collection.into(),
                provider: FILESYSTEM_PROVIDER.into(),
                source_id: relative.to_string_lossy().into_owned(),
                record_id: id.clone(),
                locator: relative,
                format: "objekte:json".into(),
                revision: crate::config::fingerprint("source-stamp/v1", &stamp),
                metadata: BTreeMap::from([
                    ("size_bytes".into(), stamp.size.to_string()),
                    ("modified_unix_nanos".into(), stamp.modified),
                ]),
            };
            by_record.insert(id, descriptor);
        }
        if let Some(extensions) = &self.extensions {
            for provider in extensions.source_providers() {
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
                    if !source_ids
                        .insert((descriptor.provider.clone(), descriptor.source_id.clone()))
                    {
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
        source.finish()?;
        Ok(by_record.into_values().collect())
    }

    /// Read and decode one previously enumerated source. The descriptor is
    /// checked against current discovery before and after the provider read.
    /// This is a public extension boundary; ordinary JSON queries keep their
    /// existing fused native reader and projection.
    pub fn read_source_leaves(&self, descriptor: &SourceDescriptor) -> Result<Vec<DecodedLeaf>> {
        let collection = descriptor.collection.as_str();
        if !self.source_descriptors(collection)?.contains(descriptor) {
            return Err(Error::Request(
                "source descriptor is stale or unknown".into(),
            ));
        }
        let mut bytes = Vec::new();
        if descriptor.provider == FILESYSTEM_PROVIDER {
            let c = self.collection(collection)?;
            let source = Source::open_scoped(&c.path, c.recursive)?;
            let path = c.path.join(&descriptor.locator);
            let mut file = source.file_relative(&descriptor.locator)?;
            let before =
                Stamp::from_metadata(&file.metadata().map_err(|error| io(&path, error))?, &path)?;
            if crate::config::fingerprint("source-stamp/v1", &before) != descriptor.revision {
                return Err(Error::Unstable(path));
            }
            file.read_to_end(&mut bytes)
                .map_err(|error| io(&path, error))?;
            let after =
                Stamp::from_metadata(&file.metadata().map_err(|error| io(&path, error))?, &path)?;
            if before != after || bytes.len() as u64 != after.size {
                return Err(Error::Unstable(path));
            }
            source.finish()?;
        } else {
            let extensions = self
                .extensions
                .as_ref()
                .ok_or_else(|| Error::Request("source provider is no longer registered".into()))?;
            let provider = extensions
                .source_providers()
                .find(|provider| provider.descriptor().identity == descriptor.provider)
                .ok_or_else(|| Error::Request("source provider is no longer registered".into()))?;
            provider.read(descriptor, &mut bytes)?;
        }
        if bytes.len() > i64::MAX as usize {
            return Err(Error::Request("source exceeds supported size".into()));
        }
        if !self.source_descriptors(collection)?.contains(descriptor) {
            return Err(Error::Unstable(descriptor.locator.clone()));
        }
        let mut leaves = Vec::new();
        let mut visit = |path: &str, scalar: ScalarValue<'_>| {
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
        };
        if let Some(registry) = &self.extensions {
            registry
                .decoder(descriptor)?
                .visit_leaves(&bytes, &mut visit)?;
        } else {
            JsonFormatDecoder::default().visit_leaves(&bytes, &mut visit)?;
        }
        Ok(leaves)
    }
}
