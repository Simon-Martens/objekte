use super::*;
use crate::model::{ArrayValue, Number, Value};
use std::collections::BTreeMap;

fn built_and_trusted_logical(
    schema: &Schema,
    bytes: &[u8],
    projection: &ScanProjection,
) -> (Value, Value) {
    use crate::inventory::RecordClass;
    use crate::repository::Payload;
    use crate::source::{SourceFormat, SourceRegistry};

    let source = SourceRegistry::builtin()
        .resolve(&SourceFormat::Json)
        .unwrap();
    let mut built = source.reader().parse_for_build_outcome(bytes).unwrap();
    finish_build_record(schema, &mut built).unwrap();
    let trusted = prepare_query_record(
        RecordClass::Logical,
        &source,
        schema,
        bytes,
        projection,
        true,
    )
    .unwrap();
    let Some(Payload::Logical(logical)) =
        materialize(&ReturnMode::Logical, Vec::new(), Some(trusted)).unwrap()
    else {
        panic!("logical payload");
    };
    (built.value, logical)
}

#[test]
fn trusted_logical_root_arrays_keep_declared_metadata_without_conversions() {
    use crate::model::LogicalType;
    use crate::reference::FieldDefinition;

    for (bytes, nullable) in [
        (b"[]".as_slice(), false),
        (b"[null,null]".as_slice(), true),
        (b"[1,2]".as_slice(), true),
    ] {
        let mut schema = Schema::default();
        schema
            .register_field(FieldDefinition {
                path: Vec::new(),
                kind: LogicalType::Array(Box::new(LogicalType::Number)),
                required: true,
                nullable: false,
                indexed: false,
                multi: true,
                element_nullable: vec![nullable],
            })
            .unwrap();
        assert!(!schema.has_converted_fields());
        let (built, trusted) =
            built_and_trusted_logical(&schema, bytes, &ScanProjection::full(false));
        assert_eq!(trusted, built);
        let Value::Array(array) = trusted else {
            panic!("root array")
        };
        assert_eq!(array.element_type, Some(LogicalType::Number));
        assert_eq!(array.nullable_elements, nullable);
    }

    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: Vec::new(),
            kind: LogicalType::Array(Box::new(LogicalType::Array(Box::new(LogicalType::Number)))),
            required: true,
            nullable: false,
            indexed: false,
            multi: true,
            element_nullable: vec![false, true],
        })
        .unwrap();
    let (built, trusted) =
        built_and_trusted_logical(&schema, b"[[],[null],[1,2]]", &ScanProjection::full(false));
    assert_eq!(trusted, built);
    let Value::Array(outer) = trusted else {
        panic!("outer array")
    };
    assert_eq!(
        outer.element_type,
        Some(LogicalType::Array(Box::new(LogicalType::Number)))
    );
    assert!(!outer.nullable_elements);
    for item in outer.values {
        let Value::Array(inner) = item else {
            panic!("inner array")
        };
        assert_eq!(inner.element_type, Some(LogicalType::Number));
        assert!(inner.nullable_elements);
    }
}

#[test]
fn trusted_logical_object_arrays_match_build_and_respect_projection() {
    use crate::inventory::RecordClass;
    use crate::model::LogicalType;
    use crate::reference::FieldDefinition;
    use crate::source::{SourceFormat, SourceRegistry};
    use std::collections::HashSet;

    let mut schema = Schema::default();
    for (path, kind, nullable) in [
        (
            vec!["values"],
            LogicalType::Array(Box::new(LogicalType::Number)),
            vec![false],
        ),
        (
            vec!["all_null"],
            LogicalType::Array(Box::new(LogicalType::Number)),
            vec![true],
        ),
        (
            vec!["populated"],
            LogicalType::Array(Box::new(LogicalType::Number)),
            vec![true],
        ),
        (
            vec!["nested"],
            LogicalType::Array(Box::new(LogicalType::Array(Box::new(LogicalType::Number)))),
            vec![false, true],
        ),
        (
            vec!["objects"],
            LogicalType::Array(Box::new(LogicalType::Object)),
            vec![false],
        ),
        (
            vec!["objects", "tags"],
            LogicalType::Array(Box::new(LogicalType::String)),
            vec![true],
        ),
    ] {
        schema
            .register_field(FieldDefinition {
                path: path.into_iter().map(str::to_owned).collect(),
                kind,
                required: false,
                nullable: false,
                indexed: false,
                multi: true,
                element_nullable: nullable,
            })
            .unwrap();
    }
    assert!(!schema.has_converted_fields());
    let bytes = br#"{"values":[],"all_null":[null,null],"populated":[1,2],"nested":[[],[null],[3]],"objects":[{"tags":[]}],"inferred":[1,null]}"#;
    let (built, trusted) = built_and_trusted_logical(&schema, bytes, &ScanProjection::full(false));
    assert_eq!(trusted, built);
    let Value::Object(fields) = &trusted else {
        panic!("object")
    };
    for name in ["values", "all_null", "populated"] {
        let Value::Array(array) = &fields[name] else {
            panic!("number array")
        };
        assert_eq!(array.element_type, Some(LogicalType::Number));
        assert_eq!(array.nullable_elements, name != "values");
    }
    let Value::Array(inferred) = &fields["inferred"] else {
        panic!("inferred array")
    };
    assert_eq!(inferred.element_type, Some(LogicalType::Number));
    assert!(inferred.nullable_elements);

    let selected = HashSet::from([
        "values".to_owned(),
        "nested".to_owned(),
        "inferred".to_owned(),
    ]);
    let projection = ScanProjection {
        top_level: Some(selected.clone()),
        collect_text: false,
        resolved_text_only: false,
    };
    let (_, projected) = built_and_trusted_logical(&schema, bytes, &projection);
    let Value::Object(mut expected) = built else {
        panic!("built object")
    };
    expected.retain(|name, _| selected.contains(name));
    assert_eq!(projected, Value::Object(expected));

    let source = SourceRegistry::builtin()
        .resolve(&SourceFormat::Json)
        .unwrap();
    for stale in [
        br#"{"values":["wrong"]}"#.as_slice(),
        br#"{"values":null}"#.as_slice(),
    ] {
        assert!(
            prepare_query_record(
                RecordClass::Logical,
                &source,
                &schema,
                stale,
                &ScanProjection::full(false),
                true
            )
            .is_ok()
        );
        let mut invalid = source
            .reader()
            .parse_for_query(stale, &ScanProjection::full(false))
            .unwrap();
        assert!(schema.coerce_record(&mut invalid.value).is_err());
    }
}

#[test]
fn recorded_validation_selects_trusted_or_validating_extraction() {
    use crate::inventory::RecordClass;
    use crate::source::{JsonDecodedScalarsReader, SourceFormat, SourceRegistry};
    use std::collections::HashSet;
    use std::sync::Arc;

    let builtin = SourceRegistry::builtin()
        .resolve(&SourceFormat::JsonDecodedScalars)
        .unwrap();
    let mut registry = SourceRegistry::default();
    registry
        .register("custom", 1, "json", Arc::new(JsonDecodedScalarsReader))
        .unwrap();
    let extension = registry
        .resolve(&SourceFormat::registered("custom"))
        .unwrap();
    let projection = ScanProjection {
        top_level: Some(HashSet::from(["wanted".into()])),
        collect_text: false,
        resolved_text_only: false,
    };
    let bytes = br#"{"wanted":5,"ignored":!}"#;

    assert!(
        prepare_query_record(
            RecordClass::Logical,
            &builtin,
            &Schema::default(),
            bytes,
            &projection,
            false,
        )
        .is_ok()
    );
    assert!(
        prepare_query_record(
            RecordClass::Logical,
            &extension,
            &Schema::default(),
            bytes,
            &projection,
            false,
        )
        .is_err()
    );
    assert!(
        prepare_query_record(
            RecordClass::RawFallback,
            &builtin,
            &Schema::default(),
            bytes,
            &projection,
            false,
        )
        .is_err()
    );
}

#[test]
fn registered_reader_expansion_and_failures_keep_typed_classes() {
    use crate::source::{SourceFormat, SourceReader, SourceRegistry};
    use crate::streaming::{ExecutionContext, ExecutionErrorKind, ExecutionLimits};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };


    struct ExpandingReader {
        calls: Arc<AtomicUsize>,
        estimate: bool,
        fail: bool,
    }


    impl SourceReader for ExpandingReader {
        fn name(&self) -> &'static str {
            "expanding"
        }

        fn frame(&self, bytes: &[u8]) -> Result<Vec<crate::inventory::ByteRange>, String> {
            Ok(vec![crate::inventory::ByteRange::checked(
                0,
                bytes.len() as u64,
                bytes.len() as u64,
            )?])
        }

        fn parse(&self, _bytes: &[u8]) -> Result<ExtractedRecord, String> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if self.fail {
                return Err("extension failed".into());
            }
            Ok(ExtractedRecord {
                value: Value::Object(BTreeMap::from([(
                    "payload".into(),
                    Value::String("x".repeat(4096)),
                )])),
                text: Vec::new(),
            })
        }

        fn estimate_query_value_bytes(
            &self,
            _bytes: &[u8],
            _projection: &ScanProjection,
        ) -> Option<u64> {
            self.estimate.then_some(4096)
        }
    }


    let context = || {
        ExecutionContext::new(
            crate::clock_context::ClockContext::new(time::OffsetDateTime::now_utc()),
            ExecutionLimits {
                extension_value_bytes: 128,
                ..Default::default()
            },
            None,
        )
    };
    for (estimate, fail, expected, expected_calls) in [
        (true, false, ExecutionErrorKind::Resource, 0),
        (false, false, ExecutionErrorKind::Resource, 1),
        (false, true, ExecutionErrorKind::Extension, 1),
    ] {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut registry = SourceRegistry::default();
        // A custom registration may use a builtin-looking ID in an empty registry.
        registry
            .register(
                "json",
                1,
                "tiny",
                Arc::new(ExpandingReader {
                    calls: calls.clone(),
                    estimate,
                    fail,
                }),
            )
            .unwrap();
        let source = registry.resolve(&SourceFormat::registered("json")).unwrap();
        assert!(source.is_extension());
        let execution = context();
        let error = prepare_query_record_with_execution(
            crate::inventory::RecordClass::Logical,
            &source,
            &Schema::default(),
            b"x",
            &ScanProjection::full(false),
            false,
            Some(&execution),
        )
        .unwrap_err();
        assert_eq!(execution.recorded_error(error).kind, expected);
        assert_eq!(calls.load(Ordering::Relaxed), expected_calls);
    }
}

#[test]
fn build_rejects_non_finite_numbers_before_indexing_nested_values() {
    for invalid in [f64::NAN, f64::NEG_INFINITY, f64::INFINITY] {
        let mut record = ExtractedRecord {
            value: Value::Object(BTreeMap::from([(
                "items".into(),
                Value::Array(ArrayValue::from_unrestricted(vec![Value::Object(
                    BTreeMap::from([("score".into(), Value::Number(Number::Float(invalid)))]),
                )])),
            )])),
            text: Vec::new(),
        };
        let error = finish_build_record(&Schema::default(), &mut record).unwrap_err();
        assert!(error.contains("items: [0]: score: non-finite number"));
    }

    let mut valid = ExtractedRecord {
        value: Value::Object(BTreeMap::from([(
            "score".into(),
            Value::Number(Number::Float(-0.0)),
        )])),
        text: Vec::new(),
    };
    finish_build_record(&Schema::default(), &mut valid).unwrap();
}
