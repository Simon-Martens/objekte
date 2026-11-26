use super::*;

use crate::index::traits::segment::maintenance::SegmentPartition;
use crate::inference::{InferenceConfig, InferenceMode, InferencePolicy};
use crate::lifecycle::Index;
use crate::model::{CollectionId, LogicalType, Value};
use crate::query::{Literal, Operator};
use crate::repository::configuration::{CollectionDefinition, RepositoryOptions};
use crate::schema::FieldDefinition;
use crate::schema::operation::{BindingOperation, RegisteredBinding};
use crate::source::test_support::{fixture_registry, line_format};

#[derive(Debug)]
struct IdentityBinding(bool);

impl BindingOperation for IdentityBinding {
    fn accepts(&self, kind: &LogicalType, operator: Operator, multi: bool) -> bool {
        *kind == LogicalType::String && operator == Operator::Eq && !multi
    }

    fn prepare_literal(&self, literal: &Literal, _: &LogicalType) -> Result<Value, String> {
        match literal {
            Literal::String(text) => Ok(Value::String(text.clone())),
            _ => Err("expected string".into()),
        }
    }

    fn map_value(&self, value: &Value) -> Option<Value> {
        Some(value.clone())
    }

    fn map_text(&self, text: &str) -> String {
        text.to_owned()
    }

    fn posting(&self) -> bool {
        true
    }

    fn complete_coverage(&self) -> bool {
        self.0
    }
}

#[test]
fn named_and_inferred_posting_metadata_survives_identity_and_dense_import() {
    let root = std::env::temp_dir().join(format!("objekte-inferred-import-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/fields.rows"),
        concat!(
            "(\"name\"=\"first\",\"tag\"=\"ok\")\n",
            "(\"name\"=\"second\",\"tag\"=\"yes\")\n",
            "(\"name\"=\"third\",\"tag\"=\"above the short limit\")\n",
        ),
    )
    .unwrap();
    let mut definition = CollectionDefinition::folder("entries", "data", line_format());
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["name".into()],
            kind: LogicalType::String,
            indexed: true,
            required: true,
            nullable: false,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    for (name, complete) in [("full", true), ("sparse", false)] {
        definition
            .schema
            .register_operation(
                vec!["name".into()],
                RegisteredBinding {
                    name: name.into(),
                    version: 1,
                    operation: Arc::new(IdentityBinding(complete)),
                },
            )
            .unwrap();
    }
    let index = Index::start_build_with_factory(
        &root,
        vec![definition],
        RepositoryOptions {
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                policy: InferencePolicy {
                    short_string_max_utf8_bytes: 4,
                    min_nonblank_records: 1,
                    min_nonblank_occurrences: 1,
                    minimum_repeated_percent: 0,
                    ..Default::default()
                },
            },
            ..Default::default()
        },
        fixture_registry(),
        Arc::clone(crate::index::traits::test_factory()),
    )
    .unwrap()
    .wait_complete()
    .unwrap();
    let view = index.view().unwrap();
    let segments = view.segments(CollectionId(0)).unwrap();
    assert_eq!(segments.len(), 1);
    let source = &segments[0];
    let path = vec!["tag".into()];
    let coverage = |segment: &dyn SegmentReader| {
        segment
            .inferred_short_string_members(&path)
            .unwrap()
            .unwrap()
            .explicit_view()
            .unwrap()
            .iter()
            .collect::<Vec<_>>()
    };
    assert_eq!(coverage(source.as_ref()), [0, 1]);
    let report = |segment: &dyn SegmentReader| {
        bincode::serialize(segment.inference_report().unwrap().unwrap().as_ref()).unwrap()
    };
    let named = |segment: &dyn SegmentReader| {
        let mut values = segment
            .components()
            .unwrap()
            .into_iter()
            .filter(|descriptor| descriptor.kind == ComponentKind::CustomPosting)
            .map(|descriptor| {
                let complete = segment
                    .named_posting_complete(&descriptor)
                    .unwrap()
                    .unwrap();
                (descriptor.binding.unwrap().into_owned(), complete)
            })
            .collect::<Vec<_>>();
        values.sort_unstable();
        values
    };
    assert_eq!(
        named(source.as_ref()),
        [("full".into(), true), ("sparse".into(), false)]
    );
    let file = view
        .file_catalog()
        .unwrap()
        .file_by_path(std::path::Path::new("data/fields.rows"))
        .unwrap()
        .unwrap()
        .id;
    let partition = SegmentPartition {
        files: HashSet::from([file]),
        slots: source
            .active
            .retired(&[crate::model::RecordSlot(0)])
            .unwrap(),
    };
    std::fs::rename(root.join("data"), root.join("detached")).unwrap();
    let identity = import(
        source.as_ref(),
        FileSegmentLayout::Identity(&source.active),
        &mut || false,
    )
    .unwrap()
    .unwrap();
    assert_eq!(report(identity.as_ref()), report(source.as_ref()));
    assert_eq!(coverage(identity.as_ref()), [0, 1]);
    assert_eq!(named(identity.as_ref()), named(source.as_ref()));
    let dense = import(
        source.as_ref(),
        FileSegmentLayout::Dense {
            partition: &partition,
            catalog: view.0.catalog.as_ref(),
        },
        &mut || false,
    )
    .unwrap()
    .unwrap();
    assert_eq!(dense.descriptor().slots, 2);
    assert_eq!(
        dense.inference_report().unwrap().unwrap().eligible_records,
        2
    );
    assert_eq!(coverage(dense.as_ref()), [0]);
    assert_eq!(named(dense.as_ref()), named(source.as_ref()));
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
