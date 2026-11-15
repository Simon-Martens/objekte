//! Deterministic runtime and persistent-schema mapping for one prototype build.

use std::collections::HashMap;

use crate::model::{CollectionId, FieldPath, LogicalType};
use crate::repository::CollectionDefinition;
use crate::source::{PreparedSource, SourceRegistry};
use crate::workload::BigramAlphabet;

use super::value_codec::{
    BindingDescriptor, BindingIdentity, BindingRole, SortConstituent, SortDefinition, SortDirection,
};
use super::{StoreError, StoreResult};

pub(crate) struct CollectionRuntime {
    pub id: CollectionId,
    pub definition: CollectionDefinition,
    pub source: PreparedSource,
    pub bindings: HashMap<BindingIdentity, u32>,
    pub binding_rows: Vec<(BindingIdentity, BindingDescriptor)>,
    pub sort_ids: HashMap<FieldPath, (u32, u32)>,
    pub sort_definitions: Vec<SortDefinition>,
    pub relation_ids: HashMap<Vec<String>, u32>,
}

pub(crate) struct RuntimeSchema {
    pub collections: Vec<CollectionRuntime>,
    pub by_name: HashMap<String, CollectionId>,
}

impl RuntimeSchema {
    pub fn new(
        definitions: &[CollectionDefinition],
        registry: &SourceRegistry,
        alphabet: BigramAlphabet,
    ) -> StoreResult<Self> {
        let mut collections = Vec::with_capacity(definitions.len());
        let mut by_name = HashMap::new();
        for (index, definition) in definitions.iter().enumerate() {
            let id = CollectionId(
                u32::try_from(index)
                    .map_err(|_| StoreError::State("too many collections".into()))?,
            );
            if by_name.insert(definition.name.clone(), id).is_some() {
                return Err(StoreError::State(format!(
                    "duplicate collection {}",
                    definition.name
                )));
            }
            let source = registry
                .resolve(&definition.format)
                .map_err(StoreError::State)?;
            let mut bindings = HashMap::new();
            let mut binding_rows = Vec::new();
            let mut next_binding = 0u32;
            let mut add_binding =
                |identity: BindingIdentity,
                 role: BindingRole,
                 kind: LogicalType,
                 multi: bool,
                 text_alphabet: Option<String>| {
                    let id = next_binding;
                    next_binding += 1;
                    let descriptor = BindingDescriptor {
                        identity: identity.clone(),
                        role,
                        kind,
                        multi,
                        text_alphabet,
                        text_segment: None,
                        fingerprint: [0; 32],
                    };
                    bindings.insert(identity.clone(), id);
                    binding_rows.push((identity, descriptor));
                };
            let mut fields = definition.schema.fields().values().collect::<Vec<_>>();
            fields.sort_by(|left, right| left.path.cmp(&right.path));
            for field in fields {
                add_binding(
                    BindingIdentity::Declared {
                        path: field.path.clone(),
                    },
                    BindingRole::Structured,
                    field.kind.clone(),
                    field.multi,
                    None,
                );
            }
            let mut custom = definition
                .schema
                .operations
                .iter()
                .filter(|(_, binding)| binding.operation.posting())
                .map(|(key, binding)| (key.clone(), binding))
                .collect::<Vec<_>>();
            custom.sort_by(|left, right| left.0.cmp(&right.0));
            for (key, binding) in custom {
                let field = definition
                    .schema
                    .fields()
                    .get(&key.0)
                    .ok_or_else(|| StoreError::State("custom posting field missing".into()))?;
                add_binding(
                    BindingIdentity::Named {
                        path: key.0,
                        name: key.1,
                    },
                    BindingRole::Structured,
                    field.kind.clone(),
                    field.multi,
                    None,
                );
                let _ = binding;
            }
            let alphabet_name = format!("{alphabet:?}").to_ascii_lowercase();
            for identity in [BindingIdentity::RawText, BindingIdentity::FallbackText] {
                add_binding(
                    identity,
                    BindingRole::Text,
                    LogicalType::String,
                    false,
                    Some(alphabet_name.clone()),
                );
            }
            let mut custom_text = definition
                .schema
                .custom
                .keys()
                .chain(
                    definition
                        .schema
                        .operations
                        .iter()
                        .filter(|(_, binding)| {
                            binding.operation.bigram() && binding.operation.complete_coverage()
                        })
                        .map(|(key, _)| key),
                )
                .map(|key| {
                    crate::repository::canonical_custom_text_key(&definition.schema, &key.0, &key.1)
                })
                .collect::<Vec<_>>();
            custom_text.sort_unstable();
            custom_text.dedup();
            for (path, name) in custom_text {
                add_binding(
                    BindingIdentity::CustomText { path, name },
                    BindingRole::Text,
                    LogicalType::String,
                    false,
                    Some(alphabet_name.clone()),
                );
            }

            let mut sort_ids = HashMap::new();
            let mut sort_definitions = Vec::new();
            for (index, path) in definition.sort_fields.iter().enumerate() {
                let ids = (
                    u32::try_from(index * 2)
                        .map_err(|_| StoreError::State("sort ID overflow".into()))?,
                    u32::try_from(index * 2 + 1)
                        .map_err(|_| StoreError::State("sort ID overflow".into()))?,
                );
                let options = definition
                    .sort_overrides
                    .get(path)
                    .map_or(definition.sort_options, |config| config.options);
                let locale = definition.sort_overrides.get(path).map_or_else(
                    || definition.sort_locale.clone(),
                    |config| config.locale.clone(),
                );
                for (id, direction) in [
                    (ids.0, SortDirection::Ascending),
                    (ids.1, SortDirection::Descending),
                ] {
                    sort_definitions.push(SortDefinition {
                        id,
                        constituents: vec![SortConstituent {
                            key: path.clone(),
                            direction,
                            options,
                        }],
                        locale: locale.clone(),
                        collation_version: super::value_codec::COLLATION_VERSION.into(),
                    });
                }
                sort_ids.insert(path.clone(), ids);
            }
            let relation_ids = definition
                .schema
                .relations
                .keys()
                .enumerate()
                .map(|(index, path)| {
                    Ok((
                        path.clone(),
                        u32::try_from(index)
                            .map_err(|_| StoreError::State("relation ordinal overflow".into()))?,
                    ))
                })
                .collect::<StoreResult<HashMap<_, _>>>()?;
            collections.push(CollectionRuntime {
                id,
                definition: definition.clone(),
                source,
                bindings,
                binding_rows,
                sort_ids,
                sort_definitions,
                relation_ids,
            });
        }
        Ok(Self {
            collections,
            by_name,
        })
    }

    pub fn collection(&self, id: CollectionId) -> StoreResult<&CollectionRuntime> {
        self.collections
            .get(id.0 as usize)
            .ok_or_else(|| StoreError::State(format!("unknown runtime collection {}", id.0)))
    }
}
