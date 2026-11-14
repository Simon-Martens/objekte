//! Stable configuration identity for managed generation compatibility.

use std::path::Path;

use sha2::{Digest, Sha256};

use crate::bigram::{BIGRAM_VERSION, FOLDED_REPRESENTATION};
use crate::repository::{CollectionDefinition, RepositoryOptions};
use crate::sorting::COLLATION_VERSION;
use crate::source::SourceRegistry;
use crate::workload::BigramAlphabet;

const CONFIGURATION_HASH_VERSION: u32 = 17;

fn path_bytes(path: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    }

    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect()
    }
}

fn feed(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}

pub fn configuration_hash(
    definitions: &[CollectionDefinition],
    options: RepositoryOptions,
    sources: &SourceRegistry,
) -> Result<[u8; 32], String> {
    configuration_hash_for_model(definitions, options, sources, crate::MODEL_VERSION)
}

fn configuration_hash_for_model(
    definitions: &[CollectionDefinition],
    options: RepositoryOptions,
    sources: &SourceRegistry,
    model_version: u32,
) -> Result<[u8; 32], String> {
    let mut hash = Sha256::new();
    hash.update(model_version.to_le_bytes());
    // Keep the identity stable for generations written before the build-settings
    // section was introduced. Their compatibility hash covers the same inputs.
    hash.update(CONFIGURATION_HASH_VERSION.to_le_bytes());
    hash.update(BIGRAM_VERSION.to_le_bytes());
    feed(&mut hash, FOLDED_REPRESENTATION.as_bytes());
    feed(&mut hash, std::env::consts::OS.as_bytes());
    feed(&mut hash, COLLATION_VERSION.as_bytes());
    hash.update([match options.bigram_alphabet {
        BigramAlphabet::Ascii => 0,
        BigramAlphabet::Unicode => 1,
    }]);
    feed(&mut hash, format!("{:?}", options.inference).as_bytes());
    for definition in definitions {
        let source = sources.resolve(&definition.format)?;
        feed(&mut hash, definition.name.as_bytes());
        feed(&mut hash, &path_bytes(&definition.source_path));
        feed(&mut hash, format!("{:?}", definition.discovery).as_bytes());
        feed(&mut hash, source.id().as_bytes());
        hash.update(source.version().to_le_bytes());
        feed(&mut hash, source.extension().as_bytes());
        feed(
            &mut hash,
            format!("{:?}/{:?}", definition.format, definition.identity).as_bytes(),
        );
        feed(&mut hash, definition.sort_locale.as_bytes());
        feed(
            &mut hash,
            format!("{:?}", definition.sort_options).as_bytes(),
        );
        let mut overrides: Vec<_> = definition.sort_overrides.iter().collect();
        overrides.sort_by_key(|item| item.0.display());
        for (path, config) in overrides {
            feed(&mut hash, path.display().as_bytes());
            feed(&mut hash, config.locale.as_bytes());
            feed(&mut hash, format!("{:?}", config.options).as_bytes());
        }
        for field in &definition.sort_fields {
            feed(&mut hash, field.display().as_bytes());
        }
        for composite in &definition.schema.composite_sorts {
            feed(&mut hash, format!("{composite:?}").as_bytes());
        }
        let mut fields: Vec<_> = definition.schema.fields.values().collect();
        fields.sort_by(|a, b| a.path.cmp(&b.path));
        for field in fields {
            feed(
                &mut hash,
                format!(
                    "{:?}|{:?}|{}|{}|{}|{}|{:?}",
                    field.path,
                    field.kind,
                    field.required,
                    field.nullable,
                    field.indexed,
                    field.multi,
                    field.element_nullable
                )
                .as_bytes(),
            );
        }
        if let Some(fingerprint) = definition.schema.source_fingerprint {
            hash.update(fingerprint);
        }
        let mut excluded: Vec<_> = definition.schema.auto_index_excluded.iter().collect();
        excluded.sort();
        for path in excluded {
            feed(
                &mut hash,
                format!("auto-index-excluded:{path:?}").as_bytes(),
            );
        }
        for relation in definition.schema.relations().values() {
            feed(&mut hash, format!("{:?}", relation).as_bytes());
        }
        let mut custom: Vec<_> = definition.schema.custom.iter().collect();
        custom.sort_by(|a, b| a.0.cmp(b.0));
        for ((path, name), binding) in custom {
            feed(
                &mut hash,
                format!(
                    "{:?}|{name}|{}|{:?}",
                    path, binding.version, binding.transform
                )
                .as_bytes(),
            );
        }
        let mut operations: Vec<_> = definition.schema.operations.iter().collect();
        operations.sort_by(|a, b| a.0.cmp(b.0));
        for ((path, name), binding) in operations {
            feed(
                &mut hash,
                format!(
                    "{:?}|{name}|{}|{}|{}|{}",
                    path,
                    binding.version,
                    binding.operation.posting(),
                    binding.operation.bigram(),
                    binding.operation.complete_coverage()
                )
                .as_bytes(),
            );
        }
        let mut defaults: Vec<_> = definition.schema.defaults.iter().collect();
        defaults.sort_by(|a, b| a.0.cmp(b.0));
        for (path, name) in defaults {
            feed(&mut hash, format!("{:?}|{name}", path).as_bytes());
        }
        if let Some(raw) = &definition.schema.raw {
            feed(
                &mut hash,
                format!(
                    "raw|{}|{}|{}",
                    raw.name,
                    raw.version,
                    raw.operation.bigram()
                )
                .as_bytes(),
            );
        }
    }
    Ok(hash.finalize().into())
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::inference::InferenceMode;
    use crate::model::{FieldPath, LogicalType, PathRoot};
    use crate::reference::{CustomBinding, FieldDefinition, Transform};
    use crate::relation::{RelationCardinality, RelationDefinition};
    use crate::source::{JsonReader, SourceFormat};

    #[test]
    fn configuration_identity_tracks_content_inputs_and_ignores_runtime_budgets() {
        let sources = SourceRegistry::builtin();
        let options = RepositoryOptions::default();
        let definition = CollectionDefinition::folder("items", "items", SourceFormat::Json);
        let base =
            configuration_hash(std::slice::from_ref(&definition), options, &sources).unwrap();
        assert_ne!(
            base,
            configuration_hash_for_model(
                std::slice::from_ref(&definition),
                options,
                &sources,
                crate::MODEL_VERSION + 1
            )
            .unwrap()
        );
        let mut changed_options = options;
        changed_options.bigram_alphabet = BigramAlphabet::Unicode;
        assert_ne!(
            base,
            configuration_hash(std::slice::from_ref(&definition), changed_options, &sources)
                .unwrap()
        );
        changed_options = options;
        changed_options.inference.mode = InferenceMode::ReportOnly;
        assert_ne!(
            base,
            configuration_hash(std::slice::from_ref(&definition), changed_options, &sources)
                .unwrap()
        );
        changed_options = options;
        changed_options.query_workers += 1;
        changed_options.verified_cache_bytes = 1024;
        assert_eq!(
            base,
            configuration_hash(std::slice::from_ref(&definition), changed_options, &sources)
                .unwrap()
        );

        let mut changed = definition.clone();
        changed
            .schema
            .register_field(FieldDefinition {
                path: vec!["title".into()],
                kind: LogicalType::String,
                required: false,
                nullable: true,
                indexed: true,
                multi: false,
                element_nullable: Vec::new(),
            })
            .unwrap();
        let with_field = configuration_hash(&[changed.clone()], options, &sources).unwrap();
        assert_ne!(base, with_field);
        let mut sorted = changed.clone();
        sorted.sort_fields.push(FieldPath {
            root: PathRoot::Data,
            segments: vec!["title".into()],
        });
        assert_ne!(
            with_field,
            configuration_hash(&[sorted], options, &sources).unwrap()
        );
        let mut related = changed.clone();
        related.schema.relations.insert(
            vec!["title".into()],
            RelationDefinition {
                path: vec!["title".into()],
                target_collection: "items".into(),
                cardinality: RelationCardinality::Many,
                reverse_alias: None,
            },
        );
        assert_ne!(
            with_field,
            configuration_hash(&[related], options, &sources).unwrap()
        );
        changed.schema.custom.insert(
            (vec!["title".into()], "fold".into()),
            CustomBinding {
                name: "fold".into(),
                version: 1,
                transform: Transform::CaseFold,
            },
        );
        let with_binding = configuration_hash(&[changed.clone()], options, &sources).unwrap();
        assert_ne!(with_field, with_binding);
        changed
            .schema
            .defaults
            .insert(vec!["title".into()], "fold".into());
        assert_ne!(
            with_binding,
            configuration_hash(&[changed], options, &sources).unwrap()
        );

        let mut reader_one = SourceRegistry::default();
        reader_one
            .register("probe", 1, "json", Arc::new(JsonReader))
            .unwrap();
        let mut reader_two = SourceRegistry::default();
        reader_two
            .register("probe", 2, "json", Arc::new(JsonReader))
            .unwrap();
        let registered =
            CollectionDefinition::folder("items", "items", SourceFormat::registered("probe"));
        assert_ne!(
            configuration_hash(std::slice::from_ref(&registered), options, &reader_one).unwrap(),
            configuration_hash(&[registered], options, &reader_two).unwrap()
        );
    }
}
