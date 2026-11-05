//! Descriptor discovery and logical decoding for registered source formats.

use super::*;
use crate::JsonFilter;
use crate::extensions::{
    DecodeOutcome, DecodedRecord, LogicalNode, ScalarValue, SourceDescriptor, SourceProvider,
    TypedValue,
};
use crate::source::FilesystemJsonProvider;


/// A logical scalar and its JSON-pointer path within one decoded record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedLeaf {
    pub path: String,
    pub value: TypedValue,
}


fn pointer_parts(path: &str) -> Result<Vec<String>> {
    if path.is_empty() {
        return Ok(Vec::new());
    }
    let Some(rest) = path.strip_prefix('/') else {
        return Err(Error::Request(format!("invalid logical pointer {path:?}")));
    };
    rest.split('/')
        .map(|part| {
            let mut decoded = String::new();
            let mut characters = part.chars();
            while let Some(character) = characters.next() {
                if character == '~' {
                    decoded.push(match characters.next() {
                        Some('0') => '~',
                        Some('1') => '/',
                        _ => {
                            return Err(Error::Request(format!(
                                "invalid logical pointer {path:?}"
                            )));
                        }
                    });
                } else {
                    decoded.push(character);
                }
            }
            Ok(decoded)
        })
        .collect()
}

fn insert_logical_node(root: &mut Value, parts: &[String], value: Value) -> Result<()> {
    let Some((part, rest)) = parts.split_first() else {
        *root = value;
        return Ok(());
    };
    if root.is_null() {
        *root = if part.parse::<usize>().is_ok() {
            Value::Array(Vec::new())
        } else {
            Value::Object(serde_json::Map::new())
        };
    }
    let next = match root {
        Value::Object(fields) => fields.entry(part).or_insert(Value::Null),
        Value::Array(items) => {
            let index = part
                .parse::<usize>()
                .map_err(|_| Error::Request(format!("logical array index {part:?} is invalid")))?;
            if index.to_string() != *part {
                return Err(Error::Request(format!(
                    "logical array index {part:?} is not canonical"
                )));
            }
            if index > items.len() {
                return Err(Error::Request(
                    "logical decoder omitted an array member; emit container nodes".into(),
                ));
            }
            if index == items.len() {
                items.push(Value::Null);
            }
            &mut items[index]
        }
        _ => {
            return Err(Error::Request(
                "logical decoder placed a child below a scalar".into(),
            ));
        }
    };
    insert_logical_node(next, rest, value)
}

fn logical_record_value(
    record: &dyn DecodedRecord,
    roots: Option<&BTreeSet<String>>,
) -> Result<(Value, usize)> {
    let mut root = Value::Object(serde_json::Map::new());
    let mut projected_nodes = 0_usize;
    let mut root_array = false;
    let mut emitted = BTreeSet::new();
    let mut scalar_paths = BTreeSet::new();
    let mut parent_paths = BTreeSet::new();
    record.visit_nodes(&mut |path, node| {
        let parts = pointer_parts(path)?;
        if path.is_empty() && matches!(node, LogicalNode::Array) {
            root_array = true;
        }
        if !emitted.insert(path.to_owned()) {
            return Err(Error::Request(format!(
                "decoder emitted duplicate logical path {path:?}"
            )));
        }
        if parent_paths.contains(path) {
            return Err(Error::Request(format!(
                "decoder emitted a parent after children at {path:?}"
            )));
        }
        let mut parent = path;
        while let Some(end) = parent.rfind('/') {
            parent = &parent[..end];
            if scalar_paths.contains(parent) {
                return Err(Error::Request(format!(
                    "decoder placed a child below a scalar at {parent:?}"
                )));
            }
            parent_paths.insert(parent.to_owned());
        }
        if matches!(node, LogicalNode::Scalar(_)) {
            scalar_paths.insert(path.to_owned());
        }
        if !root_array
            && !path.is_empty()
            && roots.is_some_and(|roots| parts.first().is_none_or(|part| !roots.contains(part)))
        {
            return Ok(());
        }
        let value = match node {
            LogicalNode::Object => Value::Object(serde_json::Map::new()),
            LogicalNode::Array => Value::Array(Vec::new()),
            LogicalNode::Scalar(ScalarValue::Null) => Value::Null,
            LogicalNode::Scalar(ScalarValue::Bool(value)) => Value::Bool(value),
            LogicalNode::Scalar(ScalarValue::Text(value)) => Value::String(value.into()),
            LogicalNode::Scalar(ScalarValue::Number(value)) => {
                let parsed: Value = serde_json::from_str(value)
                    .map_err(|_| Error::Request("decoder emitted an invalid number".into()))?;
                if !parsed.is_number() {
                    return Err(Error::Request("decoder emitted a nonnumeric number".into()));
                }
                parsed
            }
        };
        insert_logical_node(&mut root, &parts, value)?;
        projected_nodes += 1;
        Ok(())
    })?;
    Ok((root, projected_nodes))
}

/// An exact selected query may build only referenced top-level JSON subtrees.
/// Complex root paths and relation traversal keep the complete document.
pub(super) fn selected_projection_roots(
    filter: Option<&JsonFilter>,
    fields: Option<&[String]>,
    sort: &[crate::Sort],
    require_fields: bool,
) -> Option<BTreeSet<String>> {
    if (require_fields && fields.is_none())
        || filter.is_some_and(|filter| filter.has_relations() || filter.uses_status())
        || sort.iter().any(|sort| sort.field == "@status")
    {
        return None;
    }
    let mut roots = BTreeSet::new();
    for path in filter.into_iter().flat_map(JsonFilter::paths) {
        let Some(crate::JsonPathStep::Key(name)) = path.first() else {
            return None;
        };
        roots.insert(name.clone());
    }
    for field in fields
        .into_iter()
        .flatten()
        .chain(sort.iter().map(|sort| &sort.field))
    {
        if field.starts_with('@') {
            if matches!(
                field.as_str(),
                "@id" | "@created" | "@modified" | "@size" | "@status" | "@filename" | "@path"
            ) {
                continue;
            }
            return None;
        }
        let path = JsonFilter::parse_path(field).ok()?;
        let Some(crate::JsonPathStep::Key(name)) = path.first() else {
            return None;
        };
        roots.insert(name.clone());
    }
    Some(roots)
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
                if descriptor.source_id.is_empty()
                    || descriptor.revision.is_empty()
                    || descriptor.locator.as_os_str().is_empty()
                {
                    return Err(Error::Request(format!(
                        "source {} has no stable identity, locator or revision",
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
    pub(super) fn selected_snapshot_for_query(
        &mut self,
        collection: &Collection,
    ) -> Result<CustomSourceSnapshot> {
        if self.options.freshness == Freshness::MetadataOnOpen
            && let Some(snapshot) = self.custom_source_snapshot.get(&collection.name)
        {
            return Ok(snapshot.clone());
        }
        let sources = self.source_descriptors(&collection.name)?;
        self.metrics.enumerations += 1;
        let extensions = self.extensions.as_deref().ok_or_else(|| {
            Error::Request("selected sources require registered extensions".into())
        })?;
        let snapshot = CustomSourceSnapshot::new(collection, sources, extensions)?;
        if self.options.freshness == Freshness::MetadataOnOpen {
            self.custom_source_snapshot
                .insert(collection.name.clone(), snapshot.clone());
        }
        Ok(snapshot)
    }

    /// Return a pinned selected inventory only if the requested scope contains
    /// a custom record. JSON-only scopes keep their concrete picker path.
    pub(super) fn selected_route(
        &mut self,
        names: &[String],
    ) -> Result<Option<Vec<(Collection, CustomSourceSnapshot)>>> {
        if let Some(name) = names
            .iter()
            .find(|name| self.invalid_selected_collections.contains(*name))
        {
            return Err(Error::Unstable(self.model.collection(name)?.path.clone()));
        }
        let Some(extensions) = self.extensions.as_ref() else {
            return Ok(None);
        };
        if !names.iter().any(|name| {
            self.custom_source_collections.contains(name)
                || extensions
                    .source_providers()
                    .any(|provider| provider.may_select(name))
        }) {
            return Ok(None);
        }
        let mut selected = Vec::with_capacity(names.len());
        let mut has_custom = false;
        for name in names.iter().collect::<BTreeSet<_>>() {
            let collection = self.model.collection(name)?.clone();
            let snapshot = self.selected_snapshot_for_query(&collection)?;
            has_custom |= snapshot.has_custom;
            selected.push((collection, snapshot));
        }
        Ok(has_custom.then_some(selected))
    }

    pub(super) fn selected_record(
        &mut self,
        collection: &Collection,
        source: &SourceDescriptor,
        bytes: &mut Vec<u8>,
        validate: bool,
    ) -> Result<Record> {
        self.selected_record_inner(collection, source, bytes, validate, true, None)
    }

    pub(super) fn selected_record_for_evaluation(
        &mut self,
        collection: &Collection,
        source: &SourceDescriptor,
        bytes: &mut Vec<u8>,
        validate: bool,
        roots: Option<&BTreeSet<String>>,
    ) -> Result<Record> {
        self.selected_record_inner(collection, source, bytes, validate, false, roots)
    }

    fn selected_record_inner(
        &mut self,
        collection: &Collection,
        source: &SourceDescriptor,
        bytes: &mut Vec<u8>,
        validate: bool,
        compute_hash: bool,
        roots: Option<&BTreeSet<String>>,
    ) -> Result<Record> {
        self.read_selected_source_bytes_into(source, bytes)?;
        self.metrics.source_reads += 1;
        self.metrics.source_bytes += bytes.len() as u64;
        let hash = if compute_hash {
            self.metrics.hashes += 1;
            digest(bytes)
        } else {
            String::new()
        };
        let mut document = if source.format == "objekte:json"
            && source.provider == crate::source::FILESYSTEM_JSON_IDENTITY
        {
            Self::parse_value(
                &mut self.metrics,
                collection,
                &source.record_id,
                bytes,
                false,
            )
        } else {
            let decoder = self.extensions.as_ref().unwrap().decoder(source)?;
            let mut data = None;
            let outcome =
                crate::extensions::format::decode_checked(decoder, bytes, &mut |record| {
                    if data.is_some() {
                        return Err(Error::Request(
                            "format decoder emitted multiple records".into(),
                        ));
                    }
                    let selected_roots =
                        (source.format == "objekte:json").then_some(roots).flatten();
                    let (value, nodes) = logical_record_value(record, selected_roots)?;
                    self.metrics.selected_projected_nodes += nodes as u64;
                    data = Some(value);
                    Ok(())
                })?;
            self.metrics.parses += 1;
            match outcome {
                DecodeOutcome::Decoded => Document {
                    data: data.expect("checked decoder emitted one record"),
                    state: RecordState {
                        status: RecordStatus::Pending,
                        issues: Vec::new(),
                    },
                },
                DecodeOutcome::Malformed(diagnostics) => {
                    Document::malformed(&source.format, diagnostics)
                }
            }
        };
        if !document.state.status.is_malformed() {
            if collection.schema.is_none() {
                document.state.status = RecordStatus::NoSchema;
            } else if validate {
                document.validate(collection);
                self.metrics.validations += 1;
            }
        }
        let size = source
            .metadata
            .get("size_bytes")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(bytes.len() as u64);
        let modified = source
            .metadata
            .get("modified_unix_nanos")
            .filter(|value| value.parse::<i128>().is_ok())
            .cloned()
            .unwrap_or_else(|| "0".into());
        let path = source.metadata.get("path").cloned().unwrap_or_else(|| {
            let locator = if source.locator.is_absolute() {
                source.locator.clone()
            } else {
                collection.path.join(&source.locator)
            };
            locator
                .strip_prefix(&self.model.root)
                .unwrap_or(&locator)
                .to_string_lossy()
                .into_owned()
        });
        Ok(Record {
            collection: collection.name.clone(),
            id: source.record_id.clone(),
            path,
            size_bytes: size,
            modified_unix_nanos: modified,
            hash,
            raw: document.state.status.is_malformed().then(|| bytes.clone()),
            state: document.state,
            data: document.data,
        })
    }

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
                .any(|key| key.collection == collection)
            || self
                .selected_derived_indexes
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
        if changed {
            let specs = self.active_selected_specs(collection)?;
            let next = CustomSourceSnapshot::new(
                self.model.collection(collection)?,
                selected,
                self.extensions
                    .as_deref()
                    .expect("selected source extensions"),
            )?;
            let has_custom = next.has_custom;
            let previous = self
                .custom_source_snapshot
                .insert(collection.to_owned(), next.clone());
            if let Err(error) =
                self.build_selected_specs_reusing(collection, &specs, previous.as_ref())
            {
                if let Some(previous) = previous {
                    self.custom_source_snapshot
                        .insert(collection.to_owned(), previous);
                } else {
                    self.custom_source_snapshot.remove(collection);
                }
                self.invalid_selected_collections.insert(collection.into());
                self.query_cache.invalidate(collection);
                return Err(error);
            }
            self.extension_text_indexes
                .retain(|key, _| key.collection != collection || key.selection == next.fingerprint);
            self.selected_derived_indexes
                .retain(|key, _| key.collection != collection || key.selection == next.fingerprint);
            self.query_cache.invalidate(collection);
            if has_custom {
                self.custom_source_collections.insert(collection.to_owned());
            } else {
                self.custom_source_collections.remove(collection);
            }
        } else if let Some(snapshot) = self.custom_source_snapshot.get(collection) {
            if snapshot.has_custom {
                self.custom_source_collections.insert(collection.to_owned());
            } else {
                self.custom_source_collections.remove(collection);
            }
        }
        self.invalid_selected_collections.remove(collection);
        Ok(Some(self.custom_source_collections.contains(collection)))
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
            let decoder = registry.decoder(descriptor)?;
            let outcome =
                crate::extensions::format::decode_checked(decoder, &bytes, &mut callback)?;
            if let DecodeOutcome::Malformed(diagnostics) = outcome {
                return Err(Error::MalformedSource {
                    format: descriptor.format.clone(),
                    message: diagnostics[0].message.clone(),
                });
            }
        } else {
            let outcome = crate::extensions::format::decode_checked(
                crate::extensions::json_decoder(),
                &bytes,
                &mut callback,
            )?;
            if let DecodeOutcome::Malformed(diagnostics) = outcome {
                return Err(Error::MalformedSource {
                    format: descriptor.format.clone(),
                    message: diagnostics[0].message.clone(),
                });
            }
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
