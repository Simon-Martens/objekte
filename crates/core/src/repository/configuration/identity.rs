//! Stable configuration identity for managed generation compatibility.

use std::path::Path;

use sha2::{Digest, Sha256};

use super::{CollectionDeclaration, CollectionDefinition, RepositoryOptions};
use crate::index::bigram::BigramAlphabet;
use crate::index::bigram::{BIGRAM_VERSION, FOLDED_REPRESENTATION};
use crate::index::sorting::COLLATION_VERSION;
use crate::model::CollectionId;
use crate::source::registry::SourceRegistry;

const CONFIGURATION_HASH_VERSION: u32 = 17;
const CONFIGURATION_HASH_DOMAIN: u32 = 1;

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

fn feed_policy(hash: &mut Sha256, definition: &impl super::CollectionPolicy) {
    feed(hash, definition.sort_locale().as_bytes());
    feed(hash, format!("{:?}", definition.sort_options()).as_bytes());
    let mut overrides: Vec<_> = definition.sort_overrides().iter().collect();
    overrides.sort_by_key(|item| item.0.display());
    for (path, config) in overrides {
        feed(hash, path.display().as_bytes());
        feed(hash, config.locale.as_bytes());
        feed(hash, format!("{:?}", config.options).as_bytes());
    }
    for field in definition.sort_fields() {
        feed(hash, field.display().as_bytes());
    }
    for composite in &definition.schema().composite_sorts {
        feed(hash, format!("{composite:?}").as_bytes());
    }
    let mut fields: Vec<_> = definition.schema().fields.values().collect();
    fields.sort_by(|a, b| a.path.cmp(&b.path));
    for field in fields {
        feed(
            hash,
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
    if let Some(fingerprint) = definition.schema().source_fingerprint {
        hash.update(fingerprint);
    }
    let mut excluded: Vec<_> = definition.schema().auto_index_excluded.iter().collect();
    excluded.sort();
    for path in excluded {
        feed(hash, format!("auto-index-excluded:{path:?}").as_bytes());
    }
    for relation in definition.schema().relations().values() {
        feed(hash, format!("{:?}", relation).as_bytes());
    }
    let mut custom: Vec<_> = definition.schema().custom.iter().collect();
    custom.sort_by(|a, b| a.0.cmp(b.0));
    for ((path, name), binding) in custom {
        feed(
            hash,
            format!(
                "{:?}|{name}|{}|{:?}",
                path, binding.version, binding.transform
            )
            .as_bytes(),
        );
    }
    let mut operations: Vec<_> = definition.schema().operations.iter().collect();
    operations.sort_by(|a, b| a.0.cmp(b.0));
    for ((path, name), binding) in operations {
        feed(
            hash,
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
    let mut defaults: Vec<_> = definition.schema().defaults.iter().collect();
    defaults.sort_by(|a, b| a.0.cmp(b.0));
    for (path, name) in defaults {
        feed(hash, format!("{:?}|{name}", path).as_bytes());
    }
    if let Some(raw) = &definition.schema().raw {
        feed(
            hash,
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

pub fn configuration_hash(
    definitions: &[CollectionDefinition],
    options: RepositoryOptions,
    sources: &SourceRegistry,
) -> Result<[u8; 32], String> {
    let mut hash = Sha256::new();
    // Historical fixed domain value. Keep it byte-for-byte so live prepared
    // updates and desktop source-check identities remain stable.
    hash.update(CONFIGURATION_HASH_DOMAIN.to_le_bytes());
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
        feed_policy(&mut hash, definition);
    }
    Ok(hash.finalize().into())
}

/// Configuration identity for a source-neutral index. This has a distinct
/// domain from historical file configurations and contains no path or format.
pub(crate) fn programmatic_configuration_hash(
    declarations: &[CollectionDeclaration],
    options: RepositoryOptions,
    registrations: &[(CollectionId, String, u32, u32)],
) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(2u32.to_le_bytes());
    hash.update(1u32.to_le_bytes());
    hash.update(BIGRAM_VERSION.to_le_bytes());
    feed(&mut hash, FOLDED_REPRESENTATION.as_bytes());
    feed(&mut hash, std::env::consts::OS.as_bytes());
    feed(&mut hash, COLLATION_VERSION.as_bytes());
    hash.update([match options.bigram_alphabet {
        BigramAlphabet::Ascii => 0,
        BigramAlphabet::Unicode => 1,
    }]);
    feed(&mut hash, format!("{:?}", options.inference).as_bytes());
    for declaration in declarations {
        feed(&mut hash, declaration.name.as_bytes());
        feed_policy(&mut hash, declaration);
    }
    let mut registrations = registrations.to_vec();
    registrations.sort_unstable();
    for (collection, id, version, representation_version) in registrations {
        hash.update(collection.0.to_le_bytes());
        feed(&mut hash, id.as_bytes());
        hash.update(version.to_le_bytes());
        hash.update(representation_version.to_le_bytes());
    }
    hash.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::inference::InferenceMode;
    use crate::model::{FieldPath, LogicalType, PathRoot};
    use crate::schema::relations::{RelationCardinality, RelationDefinition};
    use crate::schema::{FieldDefinition, operation::CustomBinding, operation::Transform};
    use crate::source::SourceFormat;
    use crate::source::json::JsonReader;

    #[test]
    fn live_configuration_hash_matches_the_pre_restore_fixture() {
        // Expected bytes were produced at the pre-restoration execution base
        // with the same definitions/options. They prove the live identity did
        // not change while disk format versions were removed.
        let sources = SourceRegistry::builtin();
        let folder = vec![CollectionDefinition::folder(
            "files",
            "",
            SourceFormat::JsonDecodedScalars,
        )];
        let configured = vec![
            CollectionDefinition::folder("authors", "data/authors", SourceFormat::Json),
            CollectionDefinition::folder("books", "data/books", SourceFormat::JsonLines),
        ];
        let cases = [
            (
                &folder,
                RepositoryOptions::default(),
                "3d6c9a95513505be8fa182c40bd88c655dca362932233b1aee53d697586590ec",
            ),
            (
                &folder,
                RepositoryOptions {
                    bigram_alphabet: crate::index::bigram::BigramAlphabet::Unicode,
                    ..RepositoryOptions::default()
                },
                "8fed0f48d81a0838e8c264b9c5c98157fc219ea5c480b83d34257723262e2093",
            ),
            (
                &configured,
                RepositoryOptions::default(),
                "9451bad09cf9cca9287b5452ef98be53cdab8e26579d4f0334cf3531078a0f04",
            ),
            (
                &configured,
                RepositoryOptions {
                    bigram_alphabet: crate::index::bigram::BigramAlphabet::Unicode,
                    ..RepositoryOptions::default()
                },
                "bb15855855053c61c268094a65258f7568ae57283e282ee5154db4fbb892d4fb",
            ),
        ];
        for (definitions, options, expected) in cases {
            let hash = configuration_hash(definitions, options, &sources).unwrap();
            let actual = hash
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn configuration_identity_tracks_content_inputs_and_ignores_runtime_budgets() {
        let sources = SourceRegistry::builtin();
        let options = RepositoryOptions::default();
        let definition = CollectionDefinition::folder("items", "items", SourceFormat::Json);
        let base =
            configuration_hash(std::slice::from_ref(&definition), options, &sources).unwrap();
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
