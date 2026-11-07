use super::*;
use crate::model::{ArrayValue, Number, Value};
use std::collections::BTreeMap;

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
