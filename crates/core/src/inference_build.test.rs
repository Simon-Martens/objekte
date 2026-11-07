use super::*;
use crate::reference::FieldDefinition;

#[test]
fn declared_and_prohibited_paths_keep_observations_without_posting_maps() {
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["manual".into()],
            kind: LogicalType::String,
            required: false,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    schema.prohibit_auto_index(vec!["blocked".into()]).unwrap();
    let mut observer = InferenceBuilder::new(
        InferenceConfig {
            mode: InferenceMode::AutoIndex,
            ..Default::default()
        },
        true,
    )
    .unwrap();
    for (slot, text) in ["one", "two", "one"].into_iter().enumerate() {
        let value = Value::from_json(serde_json::json!({
            "manual": text, "blocked": text, "eligible": text
        }))
        .unwrap();
        observer
            .observe_record_with_schema(RecordSlot(slot as u32), &value, Some(&schema))
            .unwrap();
    }
    for name in ["manual", "blocked"] {
        let index = observer.paths[&typed_path(&[name.into()])];
        let field = &observer.fields[index];
        assert!(field.values.is_none());
        assert!(field.short_values.is_none());
        assert_eq!(
            field
                .short_distinct_without_postings
                .as_ref()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(field.present.len(), 3);
    }
    let eligible = &observer.fields[observer.paths[&typed_path(&["eligible".into()])]];
    assert_eq!(eligible.short_values.as_ref().unwrap().len(), 2);
    let state = observer
        .finish(&RoaringBitmap::from_iter(0..3), 3, &schema)
        .unwrap();
    for name in ["manual", "blocked"] {
        let field = state
            .report
            .fields
            .iter()
            .find(|field| field.path == vec![name])
            .unwrap();
        assert!(field.complete);
        assert_eq!(field.short_string_distinct_values, 2);
    }
}

#[test]
fn manual_and_opt_out_short_summaries_stop_at_existing_distinct_bounds() {
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["manual".into()],
            kind: LogicalType::String,
            required: false,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    schema.prohibit_auto_index(vec!["blocked".into()]).unwrap();
    let mut config = InferenceConfig {
        mode: InferenceMode::AutoIndex,
        ..Default::default()
    };
    config.policy.max_distinct_nonblank_strings = 2;
    config.policy.max_distinct_string_bytes = 32;
    let values = ["aa", "aa", "bb", "cc", "dd"];
    let mut reports = Vec::new();
    for partitions in [vec![5], vec![3, 2], vec![2, 2, 1]] {
        let batch_count = partitions.len();
        let mut coordinator = InferenceBuilder::new(config, false).unwrap();
        let mut cursor = 0;
        for size in partitions {
            let mut batch = InferenceBuilder::new(config, true).unwrap();
            for (local, value) in values[cursor..cursor + size].iter().enumerate() {
                let record = Value::from_json(serde_json::json!({
                    "manual": value,
                    "blocked": value,
                    "eligible": "same",
                }))
                .unwrap();
                batch
                    .observe_record_with_schema(RecordSlot(local as u32), &record, Some(&schema))
                    .unwrap();
            }
            coordinator.merge_shifted(batch, cursor as u32).unwrap();
            cursor += size;
        }
        let stats = coordinator.build_stats();
        assert_eq!(stats.observed_records, 5);
        assert_eq!(stats.merged_batches, batch_count as u64);
        assert_eq!(
            stats.repeated_batch_path_bytes,
            stats.unique_path_bytes * batch_count
        );
        assert_eq!(stats.retained_manual_summary_bytes, 0);
        assert!(stats.cumulative_budget_bytes > stats.unique_path_bytes);
        let state = coordinator
            .finish(&RoaringBitmap::from_iter(0..5), 5, &schema)
            .unwrap();
        assert!(state.report.complete);
        assert_eq!(state.report.admitted_paths, 1);
        assert!(state.indexes.contains_key(&vec!["eligible".into()]));
        for (name, status) in [
            ("manual", InferenceIndexStatus::Redundant),
            ("blocked", InferenceIndexStatus::ExplicitOptOut),
        ] {
            let report = state
                .report
                .fields
                .iter()
                .find(|report| report.path == vec![name])
                .unwrap();
            assert_eq!(report.index_status, status);
            assert!(!report.complete);
            assert_eq!(report.short_string_distinct_values, 0);
            assert!(report.reason.contains("distinct summary exceeded a bound"));
        }
        reports.push(
            state
                .report
                .fields
                .iter()
                .map(|report| {
                    (
                        report.path.clone(),
                        report.index_status,
                        report.complete,
                        report.short_string_distinct_values,
                    )
                })
                .collect::<Vec<_>>(),
        );
    }
    assert_eq!(reports[0], reports[1]);
    assert_eq!(reports[1], reports[2]);
}

#[test]
fn manual_short_summary_byte_limit_counts_unique_values_across_merge() {
    let policy = InferencePolicy {
        max_distinct_nonblank_strings: 100,
        max_distinct_string_bytes: 5,
        ..InferencePolicy::default()
    };
    let mut first = FieldState::without_provisional_postings(vec!["manual".into()]);
    first.add_scalar(RecordSlot(0), &Value::String("abc".into()), policy);
    first.add_scalar(RecordSlot(1), &Value::String("abc".into()), policy);
    assert_eq!(first.short_distinct_without_postings_bytes, 3);
    assert!(!first.short_summary_truncated);
    let mut second = FieldState::without_provisional_postings(vec!["manual".into()]);
    second.add_scalar(RecordSlot(0), &Value::String("abc".into()), policy);
    second.add_scalar(RecordSlot(1), &Value::String("xyz".into()), policy);
    first.merge_shifted(second, 2, policy).unwrap();
    assert!(first.short_summary_truncated);
    assert!(first.short_distinct_without_postings.is_none());
    assert_eq!(first.short_distinct_without_postings_bytes, 0);
    assert!(first.values.is_none());
    assert!(first.short_values.is_none());
}

#[test]
fn repeated_and_unusual_paths_reuse_names_and_traversal_storage() {
    let mut observer = InferenceBuilder::new(
        InferenceConfig {
            mode: InferenceMode::ReportOnly,
            ..Default::default()
        },
        true,
    )
    .unwrap();
    let first = Value::from_json(serde_json::json!({
        "unusual.key": {"nested": [null, {"leaf": "a"}]},
        "other": true
    }))
    .unwrap();
    observer.observe_record(RecordSlot(0), &first).unwrap();
    let names = observer.field_names.len();
    let capacity = observer.visit_path.capacity();
    assert!(capacity > 0);
    let second = Value::from_json(serde_json::json!({
        "other": false,
        "unusual.key": {"nested": [{"leaf": "b"}]}
    }))
    .unwrap();
    observer.observe_record(RecordSlot(1), &second).unwrap();
    assert_eq!(observer.field_names.len(), names);
    assert_eq!(observer.visit_path.capacity(), capacity);
    assert!(observer.visit_path.is_empty());
    observer
        .observe_record(RecordSlot(2), &Value::Null)
        .unwrap();
    assert!(observer.visit_path.is_empty());
    assert_eq!(observer.eligible, 3);
    assert_eq!(
        observer.fields[observer.paths[&typed_path(&["other".into()])]]
            .present
            .len(),
        2
    );
}

#[test]
fn distinct_bytes_track_unique_strings_through_observation_and_merge() {
    let policy = InferencePolicy::default();
    let mut left = FieldState::new(vec!["name".into()], InferenceMode::ReportOnly);
    for slot in 0..4 {
        left.add_scalar(RecordSlot(slot), &Value::String("abc".into()), policy);
    }
    assert_eq!(left.distinct_nonblank_bytes, 3);
    assert_eq!(left.distinct_nonblank.as_ref().unwrap().len(), 1);
    let mut right = FieldState::new(vec!["name".into()], InferenceMode::ReportOnly);
    right.add_scalar(RecordSlot(0), &Value::String("abc".into()), policy);
    right.add_scalar(RecordSlot(1), &Value::String("é".into()), policy);
    left.merge_shifted(right, 4, policy).unwrap();
    assert_eq!(left.distinct_nonblank_bytes, 5);
    assert_eq!(left.distinct_nonblank.as_ref().unwrap().len(), 2);
}

#[test]
fn repeated_string_at_distinct_byte_limit_keeps_summary() {
    let policy = InferencePolicy {
        max_distinct_string_bytes: 3,
        max_distinct_nonblank_strings: 1,
        ..InferencePolicy::default()
    };
    let mut field = FieldState::new(vec!["name".into()], InferenceMode::AutoIndex);
    field.add_scalar(RecordSlot(0), &Value::String("abc".into()), policy);
    assert_eq!(field.distinct_nonblank_bytes, 3);
    assert_eq!(field.distinct_nonblank.as_ref().unwrap().len(), 1);

    field.add_scalar(RecordSlot(1), &Value::String("abc".into()), policy);
    assert_eq!(field.distinct_nonblank_bytes, 3);
    assert_eq!(field.distinct_nonblank.as_ref().unwrap().len(), 1);
    assert!(!field.dropped);
    assert_eq!(field.nonblank_occurrences, 2);
    assert_eq!(field.short_values.as_ref().unwrap().len(), 1);
}

#[test]
fn multibyte_distinct_boundary_and_partitioned_merge_agree() {
    let policy = InferencePolicy {
        max_distinct_string_bytes: 3,
        max_distinct_nonblank_strings: 2,
        ..InferencePolicy::default()
    };
    let mut direct = FieldState::new(vec!["name".into()], InferenceMode::ReportOnly);
    for (slot, text) in ["é", "a", "é"].into_iter().enumerate() {
        direct.add_scalar(RecordSlot(slot as u32), &Value::String(text.into()), policy);
    }
    assert_eq!(direct.distinct_nonblank_bytes, 3);
    assert_eq!(direct.distinct_nonblank.as_ref().unwrap().len(), 2);
    assert!(!direct.dropped);

    let mut first = FieldState::new(vec!["name".into()], InferenceMode::ReportOnly);
    first.add_scalar(RecordSlot(0), &Value::String("é".into()), policy);
    let mut second = FieldState::new(vec!["name".into()], InferenceMode::ReportOnly);
    second.add_scalar(RecordSlot(0), &Value::String("a".into()), policy);
    second.add_scalar(RecordSlot(1), &Value::String("é".into()), policy);
    first.merge_shifted(second, 1, policy).unwrap();
    assert_eq!(first.distinct_nonblank, direct.distinct_nonblank);
    assert_eq!(
        first.distinct_nonblank_bytes,
        direct.distinct_nonblank_bytes
    );
    assert_eq!(first.nonblank_occurrences, direct.nonblank_occurrences);
    assert_eq!(first.nonblank_records, direct.nonblank_records);

    direct.add_scalar(RecordSlot(3), &Value::String("b".into()), policy);
    assert!(direct.distinct_nonblank.is_none());
    let mut crossing = FieldState::new(vec!["name".into()], InferenceMode::ReportOnly);
    crossing.add_scalar(RecordSlot(0), &Value::String("b".into()), policy);
    first.merge_shifted(crossing, 3, policy).unwrap();
    assert_eq!(first.distinct_nonblank, direct.distinct_nonblank);
    assert_eq!(first.dropped, direct.dropped);
}

#[test]
fn long_and_already_dropped_values_never_build_owned_keys() {
    let policy = InferencePolicy::default();
    let mut field = FieldState::new(vec!["name".into()], InferenceMode::AutoIndex);
    let long = Value::String("x".repeat(1024 * 1024));
    field.add_scalar(RecordSlot(0), &long, policy);
    assert!(field.dropped);
    assert!(field.values.is_none());
    assert_eq!(field.key_builds, 0);

    field.add_scalar(RecordSlot(1), &Value::String("short".into()), policy);
    assert_eq!(field.key_builds, 0);
    assert_eq!(field.value_occurrences, 2);

    let mut blank = FieldState::new(vec!["blank".into()], InferenceMode::AutoIndex);
    blank.add_scalar(
        RecordSlot(0),
        &Value::String(" ".repeat(policy.max_string_bytes + 1)),
        policy,
    );
    assert!(blank.dropped);
    assert_eq!(blank.key_builds, 0);

    let mut report = FieldState::new(vec!["report".into()], InferenceMode::ReportOnly);
    report.add_scalar(RecordSlot(0), &Value::String("value".into()), policy);
    assert_eq!(report.key_builds, 0);

    let mut indexed = FieldState::new(vec!["indexed".into()], InferenceMode::AutoIndex);
    indexed.add_scalar(RecordSlot(0), &Value::String("value".into()), policy);
    assert_eq!(indexed.key_builds, 0);
    assert_eq!(indexed.short_values.as_ref().unwrap().len(), 1);
}
