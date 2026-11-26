use super::projection::{
    PartitionRouting, SlotMap, project_bitmap, project_inference, project_sorting,
};
use super::*;
use crate::construction::batch::SourceFileInput;
use crate::construction::file_preparation::{FilePreparationBinding, prepare_file_build};
use crate::index::memory::bigram::BigramAlphabet;
use crate::index::memory::inference::InferenceBuilder;
use crate::index::memory::segment::admission::SegmentAdmission;
use crate::inference::{InferenceConfig, InferenceMode, InferencePolicy};
use crate::inventory::{DiagnosticRef, FileStatus, SourceRevision};
use crate::model::{CollectionId, FieldPath, LogicalType};
use crate::repository::configuration::CollectionDefinition;
use crate::schema::FieldDefinition;
use crate::schema::relations::{RelationCardinality, RelationDefinition};

fn projection_fixture_root(name: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "objekte-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    root
}

fn build_projection_fixture(
    root: &std::path::Path,
    mode: InferenceMode,
    records: &[(&str, &str)],
) -> crate::repository::Repository {
    for (name, body) in records {
        std::fs::write(root.join("data").join(name), body).unwrap();
    }
    crate::lifecycle::Index::build_repository_for_test(
        root,
        vec![crate::source::test_support::document_folder("data", "data")],
        crate::repository::configuration::RepositoryOptions {
            inference: InferenceConfig {
                mode,
                ..Default::default()
            },
            verified_cache_bytes: 0,
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap()
}

fn active_for_files(
    segment: &IndexedCollectionSegment,
    file_names: &[&str],
) -> crate::inventory::ActiveRecords {
    let selected = segment
        .records
        .iter()
        .filter_map(|record| {
            let file = segment.file(segment.backing.file_locator(record.slot)?.file)?;
            let file_name = file.entry.relative_path.file_name()?.to_str()?;
            file_names.contains(&file_name).then_some(record.slot.0)
        })
        .collect::<RoaringBitmap>();
    assert_eq!(
        selected.len(),
        file_names.len() as u64,
        "fixture file names must identify exactly one stable record each"
    );
    crate::inventory::ActiveRecords::from_bitmap(segment.record_count(), selected).unwrap()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ProjectedFieldFacts {
    reported: bool,
    complete: bool,
    present: u64,
    missing: u64,
    scalar: u64,
    array: u64,
    object: u64,
    empty_array: u64,
    empty_selection: u64,
    null_occurrence: u64,
    exact_null: u64,
    nonblank_records: u64,
    value_occurrences: u64,
    nonblank_occurrences: u64,
    short_string_observed_occurrences: u64,
    occurrence_count_status: crate::inference::OccurrenceCountStatus,
    integer_only: Option<bool>,
}

impl ProjectedFieldFacts {
    fn from_field(field: Option<&crate::inference::InferredFieldReport>, eligible: u64) -> Self {
        let Some(field) = field else {
            return Self {
                reported: false,
                complete: false,
                present: 0,
                missing: eligible,
                scalar: 0,
                array: 0,
                object: 0,
                empty_array: 0,
                empty_selection: 0,
                null_occurrence: 0,
                exact_null: 0,
                nonblank_records: 0,
                value_occurrences: 0,
                nonblank_occurrences: 0,
                short_string_observed_occurrences: 0,
                occurrence_count_status:
                    crate::inference::OccurrenceCountStatus::IncompleteObservation,
                integer_only: None,
            };
        };
        Self {
            reported: true,
            complete: field.complete,
            present: field.present_records,
            missing: field.missing_records,
            scalar: field.scalar_records,
            array: field.array_records,
            object: field.object_records,
            empty_array: field.empty_array_records,
            empty_selection: field.empty_selection_records,
            null_occurrence: field.null_occurrence_records,
            exact_null: field.exact_null_records,
            nonblank_records: field.nonblank_records,
            value_occurrences: field.value_occurrences,
            nonblank_occurrences: field.nonblank_occurrences,
            short_string_observed_occurrences: field.short_string_observed_occurrences,
            occurrence_count_status: field.occurrence_count_status,
            integer_only: field.integer_only,
        }
    }

    fn assert_matches_or_bounds(self, fresh: Self, eligible: u64, context: &str) {
        assert_eq!(
            self.present + self.missing,
            eligible,
            "{context}: presence sum"
        );
        assert!(
            self.empty_array <= self.array && self.array <= self.present,
            "{context}: array containment {self:?}"
        );
        assert!(
            self.empty_selection <= self.present,
            "{context}: empty-selection containment {self:?}"
        );
        assert!(
            self.exact_null <= self.null_occurrence && self.null_occurrence <= self.present,
            "{context}: null containment {self:?}"
        );
        assert!(
            self.nonblank_records <= self.present,
            "{context}: nonblank containment {self:?}"
        );
        assert_eq!(
            fresh.present + fresh.missing,
            eligible,
            "{context}: fresh presence sum"
        );
        assert!(
            fresh.empty_array <= fresh.array && fresh.array <= fresh.present,
            "{context}: fresh array containment {fresh:?}"
        );
        assert!(
            fresh.empty_selection <= fresh.present,
            "{context}: fresh empty-selection containment {fresh:?}"
        );
        assert!(
            fresh.exact_null <= fresh.null_occurrence && fresh.null_occurrence <= fresh.present,
            "{context}: fresh null containment {fresh:?}"
        );
        if self.complete {
            if fresh.reported {
                assert_eq!(
                    self, fresh,
                    "{context}: complete field differs from fresh build"
                );
            } else {
                assert_eq!(
                    self.present, fresh.present,
                    "{context}: complete field present"
                );
                assert_eq!(
                    self.missing, fresh.missing,
                    "{context}: complete field missing"
                );
                assert_eq!(
                    self.scalar, fresh.scalar,
                    "{context}: complete field scalar"
                );
                assert_eq!(self.array, fresh.array, "{context}: complete field array");
                assert_eq!(
                    self.object, fresh.object,
                    "{context}: complete field object"
                );
                assert_eq!(
                    self.empty_array, fresh.empty_array,
                    "{context}: complete field empty array"
                );
                assert_eq!(
                    self.empty_selection, fresh.empty_selection,
                    "{context}: complete field empty selection"
                );
                assert_eq!(
                    self.null_occurrence, fresh.null_occurrence,
                    "{context}: complete field null occurrence"
                );
                assert_eq!(
                    self.exact_null, fresh.exact_null,
                    "{context}: complete field exact null"
                );
                assert_eq!(
                    self.nonblank_records, fresh.nonblank_records,
                    "{context}: complete field nonblank records"
                );
                assert_eq!(
                    self.value_occurrences, fresh.value_occurrences,
                    "{context}: complete field value occurrences"
                );
                assert_eq!(
                    self.nonblank_occurrences, fresh.nonblank_occurrences,
                    "{context}: complete field nonblank occurrences"
                );
                assert_eq!(
                    self.short_string_observed_occurrences, fresh.short_string_observed_occurrences,
                    "{context}: complete field short-string occurrences"
                );
                assert_eq!(
                    self.integer_only, fresh.integer_only,
                    "{context}: complete field numeric evidence"
                );
            }
        } else {
            assert!(fresh.present <= self.present, "{context}: present bound");
            assert!(
                self.missing <= fresh.missing,
                "{context}: missing lower bound"
            );
            assert!(fresh.scalar <= self.scalar, "{context}: scalar bound");
            assert!(fresh.array <= self.array, "{context}: array bound");
            assert!(fresh.object <= self.object, "{context}: object bound");
            assert!(
                fresh.empty_array <= self.empty_array,
                "{context}: empty-array bound"
            );
            assert!(
                fresh.empty_selection <= self.empty_selection,
                "{context}: empty-selection bound"
            );
            assert!(
                fresh.null_occurrence <= self.null_occurrence,
                "{context}: null-occurrence bound"
            );
            assert!(
                fresh.exact_null <= self.exact_null,
                "{context}: exact-null bound"
            );
            assert!(
                fresh.nonblank_records <= self.nonblank_records,
                "{context}: nonblank-record bound"
            );
            assert_eq!(
                self.integer_only, None,
                "{context}: incomplete numeric evidence"
            );
        }
        match self.occurrence_count_status {
            crate::inference::OccurrenceCountStatus::Exact => {
                assert_eq!(
                    self.value_occurrences, fresh.value_occurrences,
                    "{context}: exact value occurrences"
                );
                assert_eq!(
                    self.nonblank_occurrences, fresh.nonblank_occurrences,
                    "{context}: exact nonblank occurrences"
                );
                assert_eq!(
                    self.short_string_observed_occurrences, fresh.short_string_observed_occurrences,
                    "{context}: exact short-string occurrences"
                );
            }
            crate::inference::OccurrenceCountStatus::UpperBound => {
                assert!(
                    fresh.value_occurrences <= self.value_occurrences,
                    "{context}: value-occurrence bound"
                );
                assert!(
                    fresh.nonblank_occurrences <= self.nonblank_occurrences,
                    "{context}: nonblank-occurrence bound"
                );
                assert!(
                    fresh.short_string_observed_occurrences
                        <= self.short_string_observed_occurrences,
                    "{context}: short-string occurrence bound"
                );
            }
            crate::inference::OccurrenceCountStatus::IncompleteObservation => {}
        }
    }
}

fn fresh_field_facts(
    report: Option<&crate::inference::InferenceReport>,
    path: &[&str],
    eligible: u64,
) -> ProjectedFieldFacts {
    let field = report.and_then(|report| {
        report.fields.iter().find(|field| {
            field.path.len() == path.len()
                && field
                    .path
                    .iter()
                    .zip(path)
                    .all(|(observed, expected)| observed == expected)
        })
    });
    ProjectedFieldFacts::from_field(field, eligible)
}

fn projected_field_facts(
    field: Option<&crate::inference::InferredFieldReport>,
    eligible: u64,
) -> ProjectedFieldFacts {
    ProjectedFieldFacts::from_field(field, eligible)
}

#[test]
fn projected_exact_null_bound_after_observation_budget_exhaustion() {
    let schema = crate::schema::Schema::default();
    let config = InferenceConfig {
        mode: InferenceMode::ReportOnly,
        policy: InferencePolicy {
            max_record_contributions: 2,
            ..Default::default()
        },
    };
    let logical = RoaringBitmap::from_iter(0..3);
    let mut builder = InferenceBuilder::for_collection(config, 1).unwrap();
    for (slot, fixture) in [
        (0, r#"("x"=1)"#),
        (1, r#"("stop"=("nested"=0))"#),
        (2, r#"("x"=null)"#),
    ] {
        builder
            .observe_record(
                RecordSlot(slot),
                &crate::source::test_support::fixture_value_from_text(fixture),
            )
            .unwrap();
    }
    let source = builder.finish(&logical, 3, &schema).unwrap();
    assert!(!source.report.complete);
    assert_eq!(source.report.eligible_records, 3);
    let source_field = source
        .report
        .fields
        .iter()
        .find(|field| field.path == ["x"])
        .unwrap();
    assert_eq!(source_field.exact_null_records, 0);
    assert!(!source.indexes.contains_key(&vec!["x".into()]));

    let slots = SlotMap::Dense(&[None, None, Some(RecordSlot(0))]);
    let projected_logical = RoaringBitmap::from_iter([0]);
    let projected = project_inference(&Some(source), &slots, &projected_logical, 1, None)
        .unwrap()
        .unwrap();
    let projected_field = projected
        .report
        .fields
        .iter()
        .find(|field| field.path == ["x"])
        .unwrap();
    assert!(!projected.report.complete);
    assert!(!projected_field.complete);
    assert_eq!(projected_field.null_occurrence_records, 1);

    let reference_logical = RoaringBitmap::from_iter([0]);
    let mut reference_builder = InferenceBuilder::for_collection(config, 1).unwrap();
    reference_builder
        .observe_record(RecordSlot(0), &fixture_value!({ "x" => null }))
        .unwrap();
    let reference = reference_builder
        .finish(&reference_logical, 1, &schema)
        .unwrap();
    assert!(reference.report.complete);
    let reference_field = reference
        .report
        .fields
        .iter()
        .find(|field| field.path == ["x"])
        .unwrap();
    assert_eq!(reference_field.exact_null_records, 1);
    assert_eq!(
        projected_field.exact_null_records,
        reference_field.exact_null_records
    );
    projected
        .validate(config, &projected_logical, 1, &schema)
        .unwrap();

    let array_config = InferenceConfig {
        mode: InferenceMode::ReportOnly,
        ..Default::default()
    };
    let mut array_builder = InferenceBuilder::for_collection(array_config, 1).unwrap();
    array_builder
        .observe_record(RecordSlot(0), &fixture_value!({ "x" => [null] }))
        .unwrap();
    let array_source = array_builder
        .finish(&reference_logical, 1, &schema)
        .unwrap();
    assert!(array_source.report.complete);
    let self_slot = SlotMap::Dense(&[Some(RecordSlot(0))]);
    let array_projected =
        project_inference(&Some(array_source), &self_slot, &reference_logical, 1, None)
            .unwrap()
            .unwrap();
    let array_field = array_projected
        .report
        .fields
        .iter()
        .find(|field| field.path == ["x"])
        .unwrap();
    assert_eq!(array_field.null_occurrence_records, 1);
    assert_eq!(array_field.exact_null_records, 0);
}

#[test]
fn projected_occurrence_counts_expose_exactness_and_observation_status() {
    use crate::inference::OccurrenceCountStatus;
    let schema = crate::schema::Schema::default();
    let interrupted_config = InferenceConfig {
        mode: InferenceMode::ReportOnly,
        policy: InferencePolicy {
            max_record_contributions: 2,
            ..Default::default()
        },
    };
    let interrupted_logical = RoaringBitmap::from_iter(0..3);
    let mut interrupted = InferenceBuilder::for_collection(interrupted_config, 1).unwrap();
    for (slot, fixture) in [
        (0, r#"("x"=null)"#),
        (1, r#"("stop"=("nested"=0))"#),
        (2, r#"("x"=["a","b"])"#),
    ] {
        interrupted
            .observe_record(
                RecordSlot(slot),
                &crate::source::test_support::fixture_value_from_text(fixture),
            )
            .unwrap();
    }
    let source = interrupted
        .finish(&interrupted_logical, 3, &schema)
        .unwrap();
    assert!(!source.report.complete);
    let source_field = source
        .report
        .fields
        .iter()
        .find(|field| field.path == ["x"])
        .unwrap();
    assert_eq!(
        source_field.occurrence_count_status,
        OccurrenceCountStatus::IncompleteObservation
    );
    assert_eq!(source_field.value_occurrences, 0);
    assert_eq!(source_field.nonblank_occurrences, 0);
    assert_eq!(source_field.short_string_observed_occurrences, 0);

    let slots = SlotMap::Dense(&[None, None, Some(RecordSlot(0))]);
    let survivor_logical = RoaringBitmap::from_iter([0]);
    let projected = project_inference(&Some(source), &slots, &survivor_logical, 1, None)
        .unwrap()
        .unwrap();
    let projected_field = projected
        .report
        .fields
        .iter()
        .find(|field| field.path == ["x"])
        .unwrap();
    assert_eq!(
        projected_field.occurrence_count_status,
        OccurrenceCountStatus::IncompleteObservation
    );
    assert_eq!(
        (
            projected_field.value_occurrences,
            projected_field.nonblank_occurrences,
            projected_field.short_string_observed_occurrences,
        ),
        (0, 0, 0)
    );
    let identity = SlotMap::Dense(&[Some(RecordSlot(0))]);
    let repeated = project_inference(
        &Some(projected.clone()),
        &identity,
        &survivor_logical,
        1,
        None,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        repeated
            .report
            .fields
            .iter()
            .find(|field| field.path == ["x"])
            .unwrap()
            .occurrence_count_status,
        OccurrenceCountStatus::IncompleteObservation
    );

    let mut fresh_builder = InferenceBuilder::for_collection(
        InferenceConfig {
            mode: InferenceMode::ReportOnly,
            ..Default::default()
        },
        1,
    )
    .unwrap();
    fresh_builder
        .observe_record(RecordSlot(0), &fixture_value!({ "x" => [("a"), ("b")] }))
        .unwrap();
    let fresh = fresh_builder.finish(&survivor_logical, 1, &schema).unwrap();
    let fresh_field = fresh
        .report
        .fields
        .iter()
        .find(|field| field.path == ["x"])
        .unwrap();
    assert_eq!(
        fresh_field.occurrence_count_status,
        OccurrenceCountStatus::Exact
    );
    assert_eq!(
        (
            fresh_field.value_occurrences,
            fresh_field.nonblank_occurrences,
            fresh_field.short_string_observed_occurrences,
        ),
        (2, 2, 2)
    );

    let empty_logical = RoaringBitmap::new();
    let empty_slots = SlotMap::Dense(&[None]);
    let empty = project_inference(&Some(fresh.clone()), &empty_slots, &empty_logical, 0, None)
        .unwrap()
        .unwrap();
    let empty_field = empty
        .report
        .fields
        .iter()
        .find(|field| field.path == ["x"])
        .unwrap();
    assert_eq!(
        empty_field.occurrence_count_status,
        OccurrenceCountStatus::Exact
    );
    assert_eq!(
        (
            empty_field.value_occurrences,
            empty_field.nonblank_occurrences,
            empty_field.short_string_observed_occurrences,
        ),
        (0, 0, 0)
    );

    let complete_logical = RoaringBitmap::from_iter(0..2);
    let mut complete = InferenceBuilder::for_collection(
        InferenceConfig {
            mode: InferenceMode::ReportOnly,
            ..Default::default()
        },
        1,
    )
    .unwrap();
    for (slot, text) in ["a", "b"].into_iter().enumerate() {
        complete
            .observe_record(RecordSlot(slot as u32), &fixture_value!({ "x" => (text) }))
            .unwrap();
    }
    let complete = complete.finish(&complete_logical, 2, &schema).unwrap();
    let upper_slots = SlotMap::Dense(&[Some(RecordSlot(0)), None]);
    let one_logical = RoaringBitmap::from_iter([0]);
    let upper = project_inference(&Some(complete), &upper_slots, &one_logical, 1, None)
        .unwrap()
        .unwrap();
    let upper_field = upper
        .report
        .fields
        .iter()
        .find(|field| field.path == ["x"])
        .unwrap();
    assert_eq!(
        upper_field.occurrence_count_status,
        OccurrenceCountStatus::UpperBound
    );
    assert!(upper_field.value_occurrences >= 1);
    assert!(upper_field.nonblank_occurrences >= 1);
}

#[test]
fn interrupted_contributor_downgrades_combined_occurrence_status() {
    use crate::inference::OccurrenceCountStatus;
    let complete_root = projection_fixture_root("occurrence-complete-contributor");
    std::fs::write(complete_root.join("data/a.row"), r#"("x"="a")"#).unwrap();
    let complete_repo = crate::lifecycle::Index::build_repository_for_test(
        &complete_root,
        vec![crate::source::test_support::document_folder("data", "data")],
        crate::repository::configuration::RepositoryOptions {
            inference: InferenceConfig {
                mode: InferenceMode::ReportOnly,
                policy: InferencePolicy {
                    max_record_contributions: 2,
                    ..Default::default()
                },
            },
            verified_cache_bytes: 0,
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let complete = complete_repo.segments[0][0].native_payload();
    let fresh_logical = RoaringBitmap::from_iter(0..3);
    let mut fresh_builder = InferenceBuilder::for_collection(
        InferenceConfig {
            mode: InferenceMode::ReportOnly,
            ..Default::default()
        },
        1,
    )
    .unwrap();
    for (slot, fixture) in [
        (0, r#"("x"="a")"#),
        (1, r#"("x"=["b","c"])"#),
        (2, r#"("other"=true)"#),
    ] {
        fresh_builder
            .observe_record(
                RecordSlot(slot),
                &crate::source::test_support::fixture_value_from_text(fixture),
            )
            .unwrap();
    }
    let fresh = fresh_builder
        .finish(&fresh_logical, 3, &crate::schema::Schema::default())
        .unwrap();
    let fresh_field = fresh
        .report
        .fields
        .iter()
        .find(|field| field.path == ["x"])
        .unwrap();
    assert_eq!(
        fresh_field.occurrence_count_status,
        OccurrenceCountStatus::Exact
    );
    assert_eq!(
        (
            fresh_field.value_occurrences,
            fresh_field.nonblank_occurrences,
            fresh_field.short_string_observed_occurrences,
        ),
        (3, 3, 3)
    );

    let interrupted_root = projection_fixture_root("occurrence-interrupted-contributor");
    std::fs::write(
        interrupted_root.join("data/b0.row"),
        r#"("stop"=("nested"=0))"#,
    )
    .unwrap();
    std::fs::write(interrupted_root.join("data/b1.row"), r#"("x"=["b","c"])"#).unwrap();
    let interrupted_repo = crate::lifecycle::Index::build_repository_for_test(
        &interrupted_root,
        vec![crate::source::test_support::document_folder("data", "data")],
        crate::repository::configuration::RepositoryOptions {
            inference: InferenceConfig {
                mode: InferenceMode::ReportOnly,
                policy: InferencePolicy {
                    max_record_contributions: 2,
                    ..Default::default()
                },
            },
            verified_cache_bytes: 0,
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let mut interrupted = interrupted_repo.segments[0][0].native_payload().clone();
    assert!(!interrupted.inference.as_ref().unwrap().report.complete);
    assert!(
        !interrupted
            .inference
            .as_ref()
            .unwrap()
            .report
            .fields
            .iter()
            .any(|field| field.path == ["x"])
    );
    let old_file = interrupted.files[0].entry.id;
    interrupted.files[0].entry.id = crate::model::FileId(10);
    interrupted.file_index.remove(&old_file);
    interrupted.file_index.insert(crate::model::FileId(10), 0);
    if let crate::index::memory::segment::SegmentBacking::FileRange { locators } =
        &mut interrupted.backing
    {
        for locator in locators {
            if locator.file == old_file {
                locator.file = crate::model::FileId(10);
            }
        }
    }

    for parts in [&[complete, &interrupted][..], &[&interrupted, complete][..]] {
        let consolidated = IndexedCollectionSegment::consolidate(parts, 3).unwrap();
        let state = consolidated.index.inference.as_ref().unwrap();
        assert!(!state.report.complete);
        let field = state
            .report
            .fields
            .iter()
            .find(|field| field.path == ["x"])
            .unwrap();
        assert_eq!(
            field.occurrence_count_status,
            OccurrenceCountStatus::IncompleteObservation
        );
        assert_eq!(
            (
                field.value_occurrences,
                field.nonblank_occurrences,
                field.short_string_observed_occurrences,
            ),
            (1, 1, 1)
        );
        let survivors = RoaringBitmap::from_iter([2]);
        let slots = SlotMap::Dense(&[None, None, Some(RecordSlot(0))]);
        let projected =
            project_inference(&consolidated.index.inference, &slots, &survivors, 1, None)
                .unwrap()
                .unwrap();
        assert_eq!(
            projected
                .report
                .fields
                .iter()
                .find(|field| field.path == ["x"])
                .unwrap()
                .occurrence_count_status,
            OccurrenceCountStatus::IncompleteObservation
        );
    }
    std::fs::remove_dir_all(complete_root).unwrap();
    std::fs::remove_dir_all(interrupted_root).unwrap();
}

struct ProjectionFixture {
    name: &'static str,
    mode: InferenceMode,
    path: &'static [&'static str],
    files: &'static [(&'static str, &'static str)],
}

#[test]
fn projected_inference_masks_match_fresh_reports_across_shapes() {
    let fixtures = [
        ProjectionFixture {
            name: "retired-empty-arrays",
            mode: InferenceMode::AutoIndex,
            path: &["x"],
            files: &[
                ("a0.row", r#"("x"=[])"#),
                ("a1.row", r#"("x"=[])"#),
                ("a2.row", r#"("x"=[7])"#),
            ],
        },
        ProjectionFixture {
            name: "mixed-shapes",
            mode: InferenceMode::AutoIndex,
            path: &["x"],
            files: &[
                ("b0.row", r#"("x"=[])"#),
                ("b1.row", r#"("x"=())"#),
                ("b2.row", r#"("x"=null)"#),
                ("b3.row", r#"("other"=true)"#),
            ],
        },
        ProjectionFixture {
            name: "report-only",
            mode: InferenceMode::ReportOnly,
            path: &["label"],
            files: &[
                ("c0.row", r#"("label"="Ada")"#),
                ("c1.row", r#"("other"=true)"#),
            ],
        },
        ProjectionFixture {
            name: "array-terminal-scalar",
            mode: InferenceMode::AutoIndex,
            path: &["items", "name"],
            files: &[
                ("n0.row", r#"("items"=[("name"="A"),("name"="B")])"#),
                ("n1.row", r#"("other"=true)"#),
            ],
        },
        ProjectionFixture {
            name: "integer-fraction",
            mode: InferenceMode::AutoIndex,
            path: &["x"],
            files: &[("i0.row", r#"("x"=1.5)"#), ("i1.row", r#"("x"=7)"#)],
        },
    ];
    for fixture in fixtures {
        let root = projection_fixture_root(fixture.name);
        let repo = build_projection_fixture(&root, fixture.mode, fixture.files);
        let segment = repo.segments[0][0].native_payload();
        assert_eq!(
            segment.record_count(),
            fixture.files.len(),
            "{}: fixture must produce one logical record per file",
            fixture.name
        );
        let config = InferenceConfig {
            mode: fixture.mode,
            ..Default::default()
        };
        for mask in 0u64..(1u64 << fixture.files.len()) {
            let selected_files = fixture
                .files
                .iter()
                .enumerate()
                .filter(|(offset, _)| mask & (1 << offset) != 0)
                .map(|(_, (name, _))| *name)
                .collect::<Vec<_>>();
            assert_eq!(
                active_for_files(segment, &selected_files).len(),
                selected_files.len() as u64,
                "{} mask {mask:#x}: survivor count",
                fixture.name
            );
            let active = active_for_files(segment, &selected_files);

            let oracle_root = root.join(format!("oracle-{mask:02}"));
            std::fs::create_dir_all(oracle_root.join("data")).unwrap();
            for (name, body) in fixture
                .files
                .iter()
                .filter(|(name, _)| selected_files.contains(name))
            {
                std::fs::write(oracle_root.join("data").join(name), body).unwrap();
            }
            let oracle = build_projection_fixture(&oracle_root, fixture.mode, &[]);
            let oracle_eligible = selected_files.len() as u64;
            let fresh = fresh_field_facts(
                oracle.inference_report("data"),
                fixture.path,
                oracle_eligible,
            );

            let compacted = segment
                .compact_active(&active, fixture.files.len(), |_| true)
                .unwrap();
            let compacted_state = compacted.index.inference.as_ref().unwrap();
            compacted_state
                .validate(
                    config,
                    &compacted.index.logical_records,
                    compacted.index.record_count(),
                    &crate::schema::Schema::default(),
                )
                .unwrap_or_else(|error| {
                    panic!(
                        "{} mask {mask:#x} path={} stage=compaction validation: {error}",
                        fixture.name,
                        fixture.path.join(".")
                    )
                });
            let compacted_field = compacted_state.report.fields.iter().find(|field| {
                field
                    .path
                    .iter()
                    .map(String::as_str)
                    .eq(fixture.path.iter().copied())
            });
            let compacted_facts =
                projected_field_facts(compacted_field, compacted.index.logical_records.len());
            compacted_facts.assert_matches_or_bounds(
                fresh,
                compacted.index.logical_records.len(),
                &format!(
                    "{} mask {mask:#x} path={} stage=compaction",
                    fixture.name,
                    fixture.path.join(".")
                ),
            );

            let routing = PartitionRouting::new(segment, &[active]).unwrap();
            let domains = segment.project_domains_for_partitions(&routing).unwrap();
            let partitioned = segment
                .project_inference_for_partitions(&routing, &domains)
                .unwrap();
            let partitioned_state = partitioned[0].as_ref().unwrap();
            partitioned_state
                .validate(
                    config,
                    &domains[0].logical,
                    routing.selected[0].len(),
                    &crate::schema::Schema::default(),
                )
                .unwrap_or_else(|error| {
                    panic!(
                        "{} mask {mask:#x} path={} stage=partition validation: {error}",
                        fixture.name,
                        fixture.path.join(".")
                    )
                });
            let partitioned_field = partitioned_state.report.fields.iter().find(|field| {
                field
                    .path
                    .iter()
                    .map(String::as_str)
                    .eq(fixture.path.iter().copied())
            });
            let partitioned_facts =
                projected_field_facts(partitioned_field, domains[0].logical.len());
            partitioned_facts.assert_matches_or_bounds(
                fresh,
                domains[0].logical.len(),
                &format!(
                    "{} mask {mask:#x} path={} stage=partition",
                    fixture.name,
                    fixture.path.join(".")
                ),
            );

            let record_count = compacted.index.record_count();
            let identity = (0..record_count as u32)
                .map(|slot| Some(RecordSlot(slot)))
                .collect::<Vec<_>>();
            let identity_slots = SlotMap::Dense(&identity);
            let second = project_inference(
                &compacted.index.inference,
                &identity_slots,
                &compacted.index.logical_records,
                record_count,
                identity_slots.bounds().unwrap(),
            )
            .unwrap()
            .unwrap();
            second
                .validate(
                    config,
                    &compacted.index.logical_records,
                    record_count,
                    &crate::schema::Schema::default(),
                )
                .unwrap_or_else(|error| {
                    panic!(
                        "{} mask {mask:#x} path={} stage=second-projection validation: {error}",
                        fixture.name,
                        fixture.path.join(".")
                    )
                });
            let second_field = second.report.fields.iter().find(|field| {
                field
                    .path
                    .iter()
                    .map(String::as_str)
                    .eq(fixture.path.iter().copied())
            });
            let second_facts =
                projected_field_facts(second_field, compacted.index.logical_records.len());
            second_facts.assert_matches_or_bounds(
                fresh,
                compacted.index.logical_records.len(),
                &format!(
                    "{} mask {mask:#x} path={} stage=second-projection",
                    fixture.name,
                    fixture.path.join(".")
                ),
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn consolidated_projected_shards_preserve_bounds_and_numeric_evidence() {
    let root = projection_fixture_root("consolidated-projected-shards");
    let mixed_files = [
        ("b0.row", r#"("x"=[])"#),
        ("b1.row", r#"("x"=())"#),
        ("b2.row", r#"("x"=null)"#),
        ("b3.row", r#"("other"=true)"#),
    ];
    let repo = build_projection_fixture(&root, InferenceMode::AutoIndex, &mixed_files);
    let segment = repo.segments[0][0].native_payload();
    let config = InferenceConfig {
        mode: InferenceMode::AutoIndex,
        ..Default::default()
    };
    let first = segment
        .compact_active(
            &active_for_files(segment, &["b0.row", "b2.row"]),
            mixed_files.len(),
            |file| {
                matches!(
                    file.relative_path
                        .file_name()
                        .and_then(std::ffi::OsStr::to_str),
                    Some("b0.row" | "b2.row")
                )
            },
        )
        .unwrap();
    let second = segment
        .compact_active(
            &active_for_files(segment, &["b1.row", "b3.row"]),
            mixed_files.len(),
            |file| {
                matches!(
                    file.relative_path
                        .file_name()
                        .and_then(std::ffi::OsStr::to_str),
                    Some("b1.row" | "b3.row")
                )
            },
        )
        .unwrap();
    let empty = segment
        .compact_active(
            &crate::inventory::ActiveRecords::from_bitmap(
                segment.record_count(),
                RoaringBitmap::new(),
            )
            .unwrap(),
            mixed_files.len(),
            |_| false,
        )
        .unwrap();
    let consolidated =
        IndexedCollectionSegment::consolidate(&[&first.index, &second.index], mixed_files.len())
            .unwrap();
    let state = consolidated.index.inference.as_ref().unwrap();
    state
        .validate(
            config,
            &consolidated.index.logical_records,
            consolidated.index.record_count(),
            &crate::schema::Schema::default(),
        )
        .unwrap();
    let field = state
        .report
        .fields
        .iter()
        .find(|field| field.path == ["x"])
        .unwrap();
    let facts = projected_field_facts(Some(field), consolidated.index.logical_records.len());
    let fresh = fresh_field_facts(
        repo.inference_report("data"),
        &["x"],
        repo.segments[0][0].native_payload().logical_records.len(),
    );
    facts.assert_matches_or_bounds(
        fresh,
        consolidated.index.logical_records.len(),
        "mixed-shapes parts=b0,b2+b1,b3 stage=consolidation",
    );
    assert_eq!(field.empty_array_records, 1);

    let with_empty =
        IndexedCollectionSegment::consolidate(&[&empty.index, &consolidated.index], 4).unwrap();
    let empty_field = with_empty
        .index
        .inference
        .as_ref()
        .unwrap()
        .report
        .fields
        .iter()
        .find(|field| field.path == ["x"])
        .unwrap();
    assert_eq!(empty_field.present_records, field.present_records);
    assert_eq!(empty_field.array_records, field.array_records);
    assert_eq!(empty_field.empty_array_records, field.empty_array_records);
    with_empty
        .index
        .inference
        .as_ref()
        .unwrap()
        .validate(
            config,
            &with_empty.index.logical_records,
            with_empty.index.record_count(),
            &crate::schema::Schema::default(),
        )
        .unwrap();

    let numeric_root = root.join("numeric");
    std::fs::create_dir_all(numeric_root.join("data")).unwrap();
    let numeric_repo = build_projection_fixture(
        &numeric_root,
        InferenceMode::AutoIndex,
        &[("i0.row", r#"("x"=1.5)"#), ("i1.row", r#"("x"=7)"#)],
    );
    let numeric_segment = numeric_repo.segments[0][0].native_payload();
    let numeric_empty = numeric_segment
        .compact_active(
            &crate::inventory::ActiveRecords::from_bitmap(
                numeric_segment.record_count(),
                RoaringBitmap::new(),
            )
            .unwrap(),
            2,
            |_| false,
        )
        .unwrap();
    let numeric_survivor = numeric_segment
        .compact_active(&active_for_files(numeric_segment, &["i1.row"]), 2, |file| {
            file.relative_path
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                == Some("i1.row")
        })
        .unwrap();
    let numeric_consolidated =
        IndexedCollectionSegment::consolidate(&[&numeric_empty.index, &numeric_survivor.index], 1)
            .unwrap();
    let numeric_state = numeric_consolidated.index.inference.as_ref().unwrap();
    numeric_state
        .validate(
            config,
            &numeric_consolidated.index.logical_records,
            numeric_consolidated.index.record_count(),
            &crate::schema::Schema::default(),
        )
        .unwrap();
    let numeric_field = numeric_state
        .report
        .fields
        .iter()
        .find(|field| field.path == ["x"])
        .unwrap();
    assert!(numeric_field.complete);
    assert_eq!(numeric_field.integer_only, Some(true));
    let numeric_facts =
        projected_field_facts(Some(numeric_field), numeric_state.report.eligible_records);
    let numeric_oracle_root = root.join("numeric-oracle");
    std::fs::create_dir_all(numeric_oracle_root.join("data")).unwrap();
    let numeric_oracle = build_projection_fixture(
        &numeric_oracle_root,
        InferenceMode::AutoIndex,
        &[("i1.row", r#"("x"=7)"#)],
    );
    let numeric_fresh = fresh_field_facts(
        numeric_oracle.inference_report("data"),
        &["x"],
        numeric_oracle.segments[0][0]
            .native_payload()
            .logical_records
            .len(),
    );
    numeric_facts.assert_matches_or_bounds(
        numeric_fresh,
        numeric_state.report.eligible_records,
        "integer-fraction parts=empty+i1 stage=consolidation",
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn projected_controls_preserve_occurrences_and_withdrawn_length_bounds() {
    let root = projection_fixture_root("projected-adversarial-controls");
    for (name, body) in [
        ("l0.row", r#"("x"=[])"#),
        ("l1.row", r#"("x"=["A","A"])"#),
        ("l2.row", r#"("x"=["A","A"])"#),
    ] {
        std::fs::write(root.join("data").join(name), body).unwrap();
    }
    let length_repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        crate::repository::configuration::RepositoryOptions {
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                policy: InferencePolicy {
                    max_distinct_values_per_path: 1,
                    ..Default::default()
                },
            },
            verified_cache_bytes: 0,
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let length_segment = length_repo.segments[0][0].native_payload();
    let original = length_segment.inference.as_ref().unwrap();
    let original_field = original
        .report
        .fields
        .iter()
        .find(|field| field.path == ["x"])
        .unwrap();
    assert_eq!(original_field.empty_array_records, 1);
    let original_posting = &original.indexes[&vec!["x".into()]];
    assert!(!original_posting.length_complete);
    assert!(original_posting.lengths.is_empty());
    let length_compacted = length_segment
        .compact_active(&active_for_files(length_segment, &["l0.row"]), 3, |_| true)
        .unwrap();
    let length_state = length_compacted.index.inference.as_ref().unwrap();
    let length_field = length_state
        .report
        .fields
        .iter()
        .find(|field| field.path == ["x"])
        .unwrap();
    assert!(!length_field.complete);
    assert_eq!(
        length_field.empty_array_records, 1,
        "withdrawn lengths: retained old empty-array bound"
    );
    assert_eq!(
        length_field.empty_selection_records, 1,
        "withdrawn lengths: retained empty-selection bound"
    );
    assert!(!length_state.indexes[&vec!["x".into()]].length_complete);
    length_state
        .validate(
            InferenceConfig {
                mode: InferenceMode::AutoIndex,
                policy: InferencePolicy {
                    max_distinct_values_per_path: 1,
                    ..Default::default()
                },
            },
            &length_compacted.index.logical_records,
            length_compacted.index.record_count(),
            &crate::schema::Schema::default(),
        )
        .unwrap();
    std::fs::remove_dir_all(&root).unwrap();

    let occurrence_root = projection_fixture_root("projected-occurrences");
    let occurrence_repo = build_projection_fixture(
        &occurrence_root,
        InferenceMode::AutoIndex,
        &[
            ("o0.row", r#"("values"=["alpha","beta"])"#),
            ("o1.row", r#"("values"=["retired"])"#),
        ],
    );
    let occurrence_segment = occurrence_repo.segments[0][0].native_payload();
    let occurrence = occurrence_segment
        .compact_active(
            &active_for_files(occurrence_segment, &["o0.row"]),
            2,
            |_| true,
        )
        .unwrap();
    let occurrence_state = occurrence.index.inference.as_ref().unwrap();
    let field = occurrence_state
        .report
        .fields
        .iter()
        .find(|field| field.path == ["values"])
        .unwrap();
    assert!(!field.complete);
    assert_eq!(field.eligible_records, 1);
    assert!(
        field.nonblank_occurrences >= 2,
        "two surviving values must remain in the occurrence bound: {field:?}"
    );
    assert!(field.nonblank_records >= 1);
    occurrence_state
        .validate(
            InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            &occurrence.index.logical_records,
            occurrence.index.record_count(),
            &crate::schema::Schema::default(),
        )
        .unwrap();
    std::fs::remove_dir_all(occurrence_root).unwrap();
}

#[test]
fn empty_unknown_array_proof_does_not_allow_nonempty_mismatches() {
    let root = projection_fixture_root("empty-unknown-array-proof");
    let repo = build_projection_fixture(
        &root,
        InferenceMode::AutoIndex,
        &[
            ("x0.row", r#"("x"=[])"#),
            ("x1.row", r#"("x"=())"#),
            ("x2.row", r#"("x"=null)"#),
            ("x3.row", r#"("other"=true)"#),
        ],
    );
    let segment = repo.segments[0][0].native_payload();
    let empty = segment
        .compact_active(&active_for_files(segment, &["x3.row"]), 4, |_| true)
        .unwrap();
    let config = InferenceConfig {
        mode: InferenceMode::AutoIndex,
        ..Default::default()
    };
    empty
        .index
        .inference
        .as_ref()
        .unwrap()
        .validate(
            config,
            &empty.index.logical_records,
            empty.index.record_count(),
            &crate::schema::Schema::default(),
        )
        .unwrap();

    let nonempty = segment
        .compact_active(&active_for_files(segment, &["x0.row"]), 4, |_| true)
        .unwrap();
    let mut invalid = nonempty.index.inference.clone().unwrap();
    let field = invalid
        .report
        .fields
        .iter_mut()
        .find(|field| field.path == ["x"])
        .unwrap();
    field
        .observed_kinds
        .retain(|kind| !matches!(kind, LogicalType::Array(_)));
    field.array_records = 0;
    field.empty_array_records = 0;
    field.empty_selection_records = 0;
    assert_eq!(
        invalid
            .validate(
                config,
                &nonempty.index.logical_records,
                nonempty.index.record_count(),
                &crate::schema::Schema::default(),
            )
            .unwrap_err(),
        "snapshot inferred posting statistics mismatch"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn surviving_empty_arrays_do_not_retain_retired_empty_array_counts() {
    let root = projection_fixture_root("surviving-empty-arrays");
    let repo = build_projection_fixture(
        &root,
        InferenceMode::AutoIndex,
        &[
            ("a0.row", r#"("x"=[])"#),
            ("a1.row", r#"("x"=[])"#),
            ("a2.row", r#"("x"=[7])"#),
        ],
    );
    let segment = repo.segments[0][0].native_payload();
    let original = segment.inference.as_ref().unwrap();
    let original_field = original
        .report
        .fields
        .iter()
        .find(|field| field.path == ["x"])
        .unwrap();
    let posting = &original.indexes[&vec!["x".into()]];
    assert!(posting.length_complete);
    assert_eq!(
        posting
            .lengths
            .get(&crate::model::Number::integer(0))
            .map_or(0, RoaringBitmap::len),
        2
    );
    assert_eq!(original_field.empty_array_records, 2);

    let compacted = segment
        .compact_active(&active_for_files(segment, &["a2.row"]), 3, |_| true)
        .unwrap();
    let state = compacted.index.inference.as_ref().unwrap();
    let field = state
        .report
        .fields
        .iter()
        .find(|field| field.path == ["x"])
        .unwrap();
    assert_eq!(field.eligible_records, 1, "A survivors=a2 eligible");
    assert_eq!(field.present_records, 1, "A survivors=a2 present");
    assert_eq!(field.missing_records, 0, "A survivors=a2 missing");
    assert_eq!(field.array_records, 1, "A survivors=a2 array records");
    assert_eq!(
        field.empty_array_records, 0,
        "A survivors=a2 empty-array records"
    );
    assert_eq!(
        field.empty_selection_records, 0,
        "A survivors=a2 empty-selection records"
    );
    state
        .validate(
            InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            &compacted.index.logical_records,
            compacted.index.record_count(),
            &crate::schema::Schema::default(),
        )
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn mixed_training_shapes_retain_projected_shape_bounds() {
    let root = projection_fixture_root("mixed-training-shapes");
    let repo = build_projection_fixture(
        &root,
        InferenceMode::AutoIndex,
        &[
            ("b0.row", r#"("x"=[])"#),
            ("b1.row", r#"("x"=())"#),
            ("b2.row", r#"("x"=null)"#),
            ("b3.row", r#"("other"=true)"#),
        ],
    );
    let segment = repo.segments[0][0].native_payload();
    let original = segment.inference.as_ref().unwrap();
    let original_field = original
        .report
        .fields
        .iter()
        .find(|field| field.path == ["x"])
        .unwrap();
    let posting = &original.indexes[&vec!["x".into()]];
    assert!(posting.length_complete);
    assert!(original_field.scalar_records >= 1);
    assert!(original_field.object_records >= 1);

    let compacted = segment
        .compact_active(
            &active_for_files(segment, &["b0.row", "b1.row", "b2.row"]),
            4,
            |_| true,
        )
        .unwrap();
    let state = compacted.index.inference.as_ref().unwrap();
    let field = state
        .report
        .fields
        .iter()
        .find(|field| field.path == ["x"])
        .unwrap();
    assert_eq!(field.eligible_records, 3, "B survivors=b0,b1,b2 eligible");
    assert_eq!(field.present_records, 3, "B survivors=b0,b1,b2 present");
    assert_eq!(field.missing_records, 0, "B survivors=b0,b1,b2 missing");
    assert_eq!(field.scalar_records, 1, "B survivors=b0,b1,b2 scalar");
    assert_eq!(field.object_records, 1, "B survivors=b0,b1,b2 object");
    assert_eq!(field.array_records, 1, "B survivors=b0,b1,b2 array");
    assert_eq!(
        field.empty_array_records, 1,
        "B survivors=b0,b1,b2 empty array"
    );
    assert_eq!(
        field.empty_selection_records, 1,
        "B survivors=b0,b1,b2 empty selection"
    );
    assert_eq!(
        field.null_occurrence_records, 1,
        "B survivors=b0,b1,b2 null occurrence"
    );
    assert_eq!(
        field.exact_null_records, 1,
        "B survivors=b0,b1,b2 exact null"
    );
    state
        .validate(
            InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            &compacted.index.logical_records,
            compacted.index.record_count(),
            &crate::schema::Schema::default(),
        )
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn report_only_projection_preserves_inherited_presence() {
    let root = projection_fixture_root("report-only-presence");
    let repo = build_projection_fixture(
        &root,
        InferenceMode::ReportOnly,
        &[
            ("c0.row", r#"("label"="Ada")"#),
            ("c1.row", r#"("other"=true)"#),
        ],
    );
    let segment = repo.segments[0][0].native_payload();
    let original = segment.inference.as_ref().unwrap();
    let original_field = original
        .report
        .fields
        .iter()
        .find(|field| field.path == ["label"])
        .unwrap();
    assert!(original_field.present_records >= 1);
    assert!(!original.indexes.contains_key(&vec!["label".into()]));

    let compacted = segment
        .compact_active(&active_for_files(segment, &["c0.row"]), 2, |_| true)
        .unwrap();
    let state = compacted.index.inference.as_ref().unwrap();
    let field = state
        .report
        .fields
        .iter()
        .find(|field| field.path == ["label"])
        .unwrap();
    assert_eq!(field.eligible_records, 1, "C survivors=c0 eligible");
    assert_eq!(field.present_records, 1, "C survivors=c0 present");
    assert_eq!(field.missing_records, 0, "C survivors=c0 missing");
    assert_eq!(field.scalar_records, 1, "C survivors=c0 scalar");
    assert_eq!(field.nonblank_records, 1, "C survivors=c0 nonblank records");
    state
        .validate(
            InferenceConfig {
                mode: InferenceMode::ReportOnly,
                ..Default::default()
            },
            &compacted.index.logical_records,
            compacted.index.record_count(),
            &crate::schema::Schema::default(),
        )
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn compaction_matches_active_file_oracle_across_indexes() {
    let root = std::env::temp_dir().join(format!(
        "objekte-segment-compact-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut definition =
        CollectionDefinition::folder("data", ".", crate::source::test_support::line_format());
    for (name, kind, indexed) in [
        ("n", LogicalType::Number, true),
        ("ref", LogicalType::Id, false),
    ] {
        definition
            .schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind,
                required: false,
                nullable: false,
                indexed,
                multi: false,
                element_nullable: vec![],
            })
            .unwrap();
    }
    definition
        .schema
        .register_relation(RelationDefinition {
            path: vec!["ref".into()],
            target_collection: "target".into(),
            cardinality: RelationCardinality::One,
            reverse_alias: None,
        })
        .unwrap();
    definition.sort_fields.push(FieldPath::parse("n").unwrap());
    definition
        .sort_fields
        .push(FieldPath::parse("ref").unwrap());
    definition
        .schema
        .register_composite_sort(vec![
            crate::query::options::SortField {
                field: "n".into(),
                descending: false,
                expression: None,
            },
            crate::query::options::SortField {
                field: "ref".into(),
                descending: true,
                expression: None,
            },
        ])
        .unwrap();
    let source = crate::source::test_support::fixture_registry()
        .resolve(&crate::source::test_support::line_format())
        .unwrap();
    let binding = FilePreparationBinding {
        factory: crate::index::storage::test_factory(),
        collection: CollectionId(0),
        definition: &definition,
        source: &source,
    };
    let inference = InferenceConfig {
        mode: InferenceMode::AutoIndex,
        policy: InferencePolicy::default(),
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let mut all = SegmentAdmission::new(CollectionId(0), &source);
    let mut only_live = SegmentAdmission::new(CollectionId(0), &source);
    for (id, name, bytes) in [
        (
            FileId(100),
            "retired.rows",
            b"(\"n\"=1,\"extra\"=11,\"ref\"=\"x\")\n(bad)\n".as_slice(),
        ),
        (
            FileId(200),
            "live.rows",
            b"(\"n\"=2,\"extra\"=22,\"ref\"=\"y\")\n".as_slice(),
        ),
    ] {
        let path = root.join(name);
        std::fs::write(&path, bytes).unwrap();
        let input = SourceFileInput {
            id,
            relative_path: name.into(),
            revision: SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap(),
            created_ns: None,
        };
        let prepared = prepare_file_build(
            &root,
            input.clone(),
            &[binding],
            BigramAlphabet::Ascii,
            inference,
            1,
        )
        .unwrap();
        all.admit(&input, prepared.collections.into_iter().next().unwrap())
            .unwrap();
        if id == FileId(200) {
            let prepared = prepare_file_build(
                &root,
                input.clone(),
                &[binding],
                BigramAlphabet::Ascii,
                inference,
                1,
            )
            .unwrap();
            only_live
                .admit(&input, prepared.collections.into_iter().next().unwrap())
                .unwrap();
        }
    }
    let all = all
        .finish()
        .into_indexed(&definition, BigramAlphabet::Ascii, inference, 1, &pool)
        .unwrap();
    let oracle = only_live
        .finish()
        .into_indexed(&definition, BigramAlphabet::Ascii, inference, 1, &pool)
        .unwrap();
    let masks = [
        crate::inventory::ActiveRecords::from_bitmap(3, RoaringBitmap::from_iter([0, 2])).unwrap(),
        crate::inventory::ActiveRecords::from_bitmap(3, RoaringBitmap::from_iter([1])).unwrap(),
    ];
    let routing = PartitionRouting::new(&all, &masks).unwrap();
    let scattered = all.project_sorting_for_partitions(&routing).unwrap();
    let domains = all.project_domains_for_partitions(&routing).unwrap();
    let scattered_inference = all
        .project_inference_for_partitions(&routing, &domains)
        .unwrap();
    for (part, sorting) in scattered.iter().enumerate() {
        let slot_map = SlotMap::Partition {
            routing: &routing,
            part,
        };
        assert_eq!(
            domains[part].logical,
            project_bitmap(&all.logical_records, &slot_map).unwrap()
        );
        assert_eq!(
            domains[part].empty_array,
            project_bitmap(&all.empty_array_records, &slot_map).unwrap()
        );
        let records = routing.selected[part]
            .iter()
            .enumerate()
            .map(|(new, &old)| {
                let mut record = all.records[old as usize].clone();
                record.slot = RecordSlot(new as u32);
                record
            })
            .collect::<Vec<_>>();
        let reference_inference = project_inference(
            &all.inference,
            &slot_map,
            &domains[part].logical,
            records.len(),
            slot_map.bounds().unwrap(),
        )
        .unwrap();
        let actual_inference = scattered_inference[part].as_ref().unwrap();
        let expected_inference = reference_inference.as_ref().unwrap();
        assert_eq!(
            bincode::serialize(&actual_inference.report).unwrap(),
            bincode::serialize(&expected_inference.report).unwrap(),
        );
        assert_eq!(
            actual_inference.short_string_coverage,
            expected_inference.short_string_coverage
        );
        assert_eq!(
            actual_inference.indexes.len(),
            expected_inference.indexes.len()
        );
        for (path, actual) in &actual_inference.indexes {
            let expected = &expected_inference.indexes[path];
            assert_eq!(actual.presence, expected.presence);
            assert_eq!(actual.null_posting(), expected.null_posting());
            assert_eq!(actual.values, expected.values);
            assert_eq!(actual.lengths, expected.lengths);
            assert_eq!(actual.summary, expected.summary);
        }
        let reference = project_sorting(&all.sorting, &slot_map, &records).unwrap();
        for (path, column) in &sorting.columns {
            let expected = &reference.columns[path];
            assert_eq!(column.ascending, expected.ascending);
            assert_eq!(column.descending, expected.descending);
        }
        for (actual, expected) in sorting.composites.iter().zip(&reference.composites) {
            assert_eq!(actual.order, expected.order);
        }
    }
    std::fs::remove_dir_all(root).unwrap();
    let active = crate::inventory::ActiveRecords::all(3)
        .unwrap()
        .retired(&[RecordSlot(0), RecordSlot(1)])
        .unwrap();
    let compacted = all
        .compact_active(&active, 3, |file| file.id == FileId(200))
        .unwrap();
    assert_eq!(compacted.old_to_new, [None, None, Some(RecordSlot(0))]);
    assert_eq!(compacted.active.len(), 1);
    let compacted = compacted.index;
    assert_eq!(compacted.records.len(), 1);
    assert_eq!(compacted.files.len(), 1);
    assert_eq!(compacted.files[0].entry.id, FileId(200));
    assert_eq!(compacted.id_lookup, oracle.id_lookup);
    assert_eq!(compacted.logical_records, oracle.logical_records);
    assert_eq!(
        compacted.postings.fields[&vec!["n".to_owned()]].values,
        oracle.postings.fields[&vec!["n".to_owned()]].values
    );
    assert_eq!(
        compacted.postings.fields[&vec!["n".to_owned()]].summary,
        oracle.postings.fields[&vec!["n".to_owned()]].summary,
    );
    assert_eq!(compacted.raw.normal, oracle.raw.normal);
    assert_eq!(compacted.fallback_text.normal, oracle.fallback_text.normal);
    assert_eq!(
        compacted.sorting.columns[&FieldPath::parse("n").unwrap()].ascending,
        oracle.sorting.columns[&FieldPath::parse("n").unwrap()].ascending
    );
    assert_eq!(
        compacted.sorting.composites[0].order,
        oracle.sorting.composites[0].order
    );
    assert_eq!(
        compacted.relations[0].references().collect::<Vec<_>>(),
        oracle.relations[0].references().collect::<Vec<_>>()
    );
    let inferred = compacted.inference.as_ref().unwrap();
    inferred
        .validate(
            inference,
            &compacted.logical_records,
            compacted.records.len(),
            &definition.schema,
        )
        .unwrap();
    assert_eq!(
        inferred.indexes[&vec!["extra".to_owned()]].values,
        oracle.inference.as_ref().unwrap().indexes[&vec!["extra".to_owned()]].values
    );
    assert_eq!(
        inferred.indexes[&vec!["extra".to_owned()]].summary,
        oracle.inference.as_ref().unwrap().indexes[&vec!["extra".to_owned()]].summary,
    );
    assert!(
        all.compact_active(&active, 2, |file| file.id == FileId(200))
            .is_err()
    );
}

#[test]
fn projected_inference_reports_keep_diagnostic_meaning_and_incompleteness() {
    let root = std::env::temp_dir().join(format!(
        "objekte-inference-report-projection-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, body) in [
        ("00_null_a", r#"("x"=[null])"#),
        ("01_null_b", r#"("x"=[null])"#),
        ("10_value", r#"("x"=[7])"#),
        ("20_missing_a", "()"),
        ("21_missing_b", "()"),
    ] {
        std::fs::write(root.join(format!("data/{name}.row")), body).unwrap();
    }
    let config = InferenceConfig {
        mode: InferenceMode::AutoIndex,
        ..Default::default()
    };
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        crate::repository::configuration::RepositoryOptions {
            inference: config,
            verified_cache_bytes: 0,
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let segment = repo.segments[0][0].native_payload();
    let active = crate::inventory::ActiveRecords::all(segment.record_count())
        .unwrap()
        .retired(&[RecordSlot(0), RecordSlot(1)])
        .unwrap();
    let compacted = segment.compact_active(&active, 5, |_| true).unwrap();
    let report = compacted
        .index
        .inference
        .as_ref()
        .unwrap()
        .report
        .fields
        .iter()
        .find(|field| field.path == ["x"])
        .unwrap();
    assert!(!report.complete);
    assert_eq!(report.present_records, 1);
    assert_eq!(report.exact_null_records, 0);
    assert_eq!(report.empty_selection_records, 0);
    assert!(report.null_occurrence_records <= report.present_records);
    assert!(
        compacted
            .index
            .inference
            .as_ref()
            .is_some_and(|state| state.indexes.contains_key(&vec!["x".into()]))
    );
    let inference = compacted.index.inference.as_ref().unwrap();
    let posting = &inference.indexes[&vec!["x".into()]];
    let crate::query::binding::filter::BoundExpr::Term(term) =
        crate::query::binding::filter::bind_filter(
            &crate::query::parse_filter("x:null = true").unwrap(),
            &crate::schema::Schema::default(),
        )
        .unwrap()
    else {
        panic!("expected term");
    };
    assert!(
        posting
            .exact_set(&term, &RoaringBitmap::from_iter(0..3))
            .unwrap()
            .is_empty()
    );
    inference
        .validate(
            config,
            &compacted.index.logical_records,
            compacted.index.record_count(),
            &crate::schema::Schema::default(),
        )
        .unwrap();
    let consolidated =
        IndexedCollectionSegment::consolidate(&[&compacted.index], compacted.index.record_count())
            .unwrap();
    let consolidated_field = consolidated
        .index
        .inference
        .as_ref()
        .unwrap()
        .report
        .fields
        .iter()
        .find(|field| field.path == ["x"])
        .unwrap();
    assert!(!consolidated_field.complete);
    consolidated
        .index
        .inference
        .as_ref()
        .unwrap()
        .validate(
            config,
            &consolidated.index.logical_records,
            consolidated.index.record_count(),
            &crate::schema::Schema::default(),
        )
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn projected_empty_selection_bound_covers_ancestor_and_terminal_arrays() {
    let root = std::env::temp_dir().join(format!(
        "objekte-inference-empty-bound-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, body) in [
        ("00_ancestor", r#"("items"=[])"#),
        ("01_terminal", r#"("items"=[("x"=[])])"#),
        ("02_retired", "()"),
    ] {
        std::fs::write(root.join(format!("data/{name}.row")), body).unwrap();
    }
    let config = InferenceConfig {
        mode: InferenceMode::AutoIndex,
        ..Default::default()
    };
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        crate::repository::configuration::RepositoryOptions {
            inference: config,
            verified_cache_bytes: 0,
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let segment = repo.segments[0][0].native_payload();
    let original = segment.inference.as_ref().unwrap();
    let original_field = original
        .report
        .fields
        .iter()
        .find(|field| field.path == ["items", "x"])
        .unwrap();
    assert_eq!(original_field.empty_selection_records, 2);
    let terminal_zero = original.indexes[&vec!["items".into(), "x".into()]]
        .lengths
        .get(&crate::model::Number::integer(0))
        .map_or(0, RoaringBitmap::len);
    assert_eq!(terminal_zero, 1);

    let active = crate::inventory::ActiveRecords::all(segment.record_count())
        .unwrap()
        .retired(&[RecordSlot(2)])
        .unwrap();
    let compacted = segment.compact_active(&active, 3, |_| true).unwrap();
    let inferred = compacted.index.inference.as_ref().unwrap();
    let field = inferred
        .report
        .fields
        .iter()
        .find(|field| field.path == ["items", "x"])
        .unwrap();
    assert!(!field.complete);
    assert_eq!(field.empty_selection_records, 2);
    inferred
        .validate(
            config,
            &compacted.index.logical_records,
            compacted.index.record_count(),
            &crate::schema::Schema::default(),
        )
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn projected_empty_short_string_coverage_is_removed_consistently() {
    let root = std::env::temp_dir().join(format!(
        "objekte-inference-empty-coverage-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/00_covered.row"), r#"("x"="A")"#).unwrap();
    std::fs::write(root.join("data/01_missing.row"), "()").unwrap();
    let config = InferenceConfig {
        mode: InferenceMode::AutoIndex,
        policy: InferencePolicy {
            min_nonblank_occurrences: 1,
            min_nonblank_records: 1,
            minimum_repeated_percent: 0,
            ..Default::default()
        },
    };
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        crate::repository::configuration::RepositoryOptions {
            inference: config,
            verified_cache_bytes: 0,
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let segment = repo.segments[0][0].native_payload();
    assert!(
        segment
            .inference
            .as_ref()
            .unwrap()
            .short_string_coverage
            .contains_key(&vec!["x".into()])
    );
    let active = crate::inventory::ActiveRecords::all(segment.record_count())
        .unwrap()
        .retired(&[RecordSlot(0)])
        .unwrap();
    let compacted = segment.compact_active(&active, 2, |_| true).unwrap();
    let inference = compacted.index.inference.as_ref().unwrap();
    assert!(
        !inference
            .short_string_coverage
            .contains_key(&vec!["x".into()])
    );
    let field = inference
        .report
        .fields
        .iter()
        .find(|field| field.path == ["x"])
        .unwrap();
    assert!(!field.complete);
    assert_eq!(field.short_string_covered_records, 0);
    assert_eq!(field.short_string_distinct_values, 0);
    inference
        .validate(
            config,
            &compacted.index.logical_records,
            compacted.index.record_count(),
            &crate::schema::Schema::default(),
        )
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn projected_terminal_scalar_counts_do_not_follow_path_multiplicity() {
    let root = std::env::temp_dir().join(format!(
        "objekte-inference-terminal-scalars-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/00_target.row"),
        r#"("items"=[("name"="A"),("name"="B")])"#,
    )
    .unwrap();
    let retired = root.join("data/01_retired.row");
    std::fs::write(&retired, r#"("other"=true)"#).unwrap();
    let config = InferenceConfig {
        mode: InferenceMode::AutoIndex,
        policy: InferencePolicy {
            min_nonblank_occurrences: 1,
            min_nonblank_records: 1,
            minimum_repeated_percent: 0,
            ..Default::default()
        },
    };
    let options = crate::repository::configuration::RepositoryOptions {
        inference: config,
        verified_cache_bytes: 0,
        ..Default::default()
    };
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        options,
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let segment = repo.segments[0][0].native_payload();
    let active = crate::inventory::ActiveRecords::all(segment.record_count())
        .unwrap()
        .retired(&[RecordSlot(1)])
        .unwrap();
    let compacted = segment.compact_active(&active, 2, |_| true).unwrap();
    drop(repo);
    std::fs::remove_file(retired).unwrap();
    let fresh = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        options,
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let path: Vec<String> = vec!["items".into(), "name".into()];
    let field = compacted
        .index
        .inference
        .as_ref()
        .unwrap()
        .report
        .fields
        .iter()
        .find(|field| field.path == path)
        .unwrap();
    let expected = fresh
        .inference_report("data")
        .unwrap()
        .fields
        .iter()
        .find(|field| field.path == path)
        .unwrap();
    assert!(!field.complete);
    assert_eq!(field.scalar_records, expected.scalar_records);
    assert_eq!(field.array_records, expected.array_records);
    assert!(field.scalar_records >= 1);
    compacted
        .index
        .inference
        .as_ref()
        .unwrap()
        .validate(
            config,
            &compacted.index.logical_records,
            compacted.index.record_count(),
            &crate::schema::Schema::default(),
        )
        .unwrap();
    let consolidated =
        IndexedCollectionSegment::consolidate(&[&compacted.index], compacted.index.record_count())
            .unwrap();
    let consolidated_field = consolidated
        .index
        .inference
        .as_ref()
        .unwrap()
        .report
        .fields
        .iter()
        .find(|field| field.path == path)
        .unwrap();
    assert!(!consolidated_field.complete);
    assert_eq!(consolidated_field.scalar_records, expected.scalar_records);
    assert_eq!(consolidated_field.array_records, expected.array_records);
    consolidated
        .index
        .inference
        .as_ref()
        .unwrap()
        .validate(
            config,
            &consolidated.index.logical_records,
            consolidated.index.record_count(),
            &crate::schema::Schema::default(),
        )
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn projected_numeric_report_recomputes_integer_only() {
    let root = std::env::temp_dir().join(format!(
        "objekte-inference-integer-only-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let fractional = root.join("data/00_fractional.row");
    std::fs::write(&fractional, r#"("x"=1.5)"#).unwrap();
    std::fs::write(root.join("data/01_integer.row"), r#"("x"=7)"#).unwrap();
    let config = InferenceConfig {
        mode: InferenceMode::AutoIndex,
        ..Default::default()
    };
    let options = crate::repository::configuration::RepositoryOptions {
        inference: config,
        verified_cache_bytes: 0,
        ..Default::default()
    };
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        options,
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let segment = repo.segments[0][0].native_payload();
    let active = crate::inventory::ActiveRecords::all(segment.record_count())
        .unwrap()
        .retired(&[RecordSlot(0)])
        .unwrap();
    let compacted = segment.compact_active(&active, 2, |_| true).unwrap();
    drop(repo);
    std::fs::remove_file(fractional).unwrap();
    let fresh = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        options,
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let field = compacted
        .index
        .inference
        .as_ref()
        .unwrap()
        .report
        .fields
        .iter()
        .find(|field| field.path == ["x"])
        .unwrap();
    let expected = fresh.inference_report("data").unwrap();
    let expected = expected
        .fields
        .iter()
        .find(|field| field.path == ["x"])
        .unwrap();
    assert!(field.complete);
    assert_eq!(field.integer_only, Some(true));
    assert_eq!(field.integer_only, expected.integer_only);
    assert_eq!(field.present_records, expected.present_records);
    assert_eq!(field.value_occurrences, expected.value_occurrences);
    let consolidated =
        IndexedCollectionSegment::consolidate(&[&compacted.index], compacted.index.record_count())
            .unwrap();
    let consolidated_field = consolidated
        .index
        .inference
        .as_ref()
        .unwrap()
        .report
        .fields
        .iter()
        .find(|field| field.path == ["x"])
        .unwrap();
    assert!(consolidated_field.complete);
    assert_eq!(consolidated_field.integer_only, Some(true));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn projected_numeric_report_with_no_survivors_does_not_retain_integer_only() {
    let root = std::env::temp_dir().join(format!(
        "objekte-inference-empty-numeric-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/00_fractional.row"), r#"("x"=1.5)"#).unwrap();
    std::fs::write(root.join("data/01_integer.row"), r#"("x"=7)"#).unwrap();
    let config = InferenceConfig {
        mode: InferenceMode::AutoIndex,
        ..Default::default()
    };
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        crate::repository::configuration::RepositoryOptions {
            inference: config,
            verified_cache_bytes: 0,
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let segment = repo.segments[0][0].native_payload();
    let active = crate::inventory::ActiveRecords::all(segment.record_count())
        .unwrap()
        .retired(&[RecordSlot(0), RecordSlot(1)])
        .unwrap();
    let compacted = segment.compact_active(&active, 2, |_| true).unwrap();
    let field = compacted
        .index
        .inference
        .as_ref()
        .unwrap()
        .report
        .fields
        .iter()
        .find(|field| field.path == ["x"])
        .unwrap();
    assert!(field.complete);
    assert_eq!(field.present_records, 0);
    assert_eq!(field.integer_only, None);
    compacted
        .index
        .inference
        .as_ref()
        .unwrap()
        .validate(
            config,
            &compacted.index.logical_records,
            compacted.index.record_count(),
            &crate::schema::Schema::default(),
        )
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn historical_file_versions_compact_to_current_zero_record_or_delete() {
    let root =
        std::env::temp_dir().join(format!("objekte-segment-versions-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("same.rows");
    let definition =
        CollectionDefinition::folder("data", ".", crate::source::test_support::line_format());
    let source = crate::source::test_support::fixture_registry()
        .resolve(&crate::source::test_support::line_format())
        .unwrap();
    let binding = FilePreparationBinding {
        factory: crate::index::storage::test_factory(),
        collection: CollectionId(0),
        definition: &definition,
        source: &source,
    };
    let inference = InferenceConfig {
        mode: InferenceMode::Disabled,
        policy: InferencePolicy::default(),
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let mut versions = Vec::new();
    for bytes in [
        b"(\"n\"=1)\n(\"n\"=2)\n".as_slice(),
        b"(\"n\"=3)\n".as_slice(),
        b"".as_slice(),
    ] {
        std::fs::write(&path, bytes).unwrap();
        let input = SourceFileInput {
            id: FileId(77),
            relative_path: "same.rows".into(),
            revision: SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap(),
            created_ns: None,
        };
        let prepared = prepare_file_build(
            &root,
            input.clone(),
            &[binding],
            BigramAlphabet::Ascii,
            inference,
            1,
        )
        .unwrap();
        let mut admission = SegmentAdmission::new(CollectionId(0), &source);
        admission
            .admit(&input, prepared.collections.into_iter().next().unwrap())
            .unwrap();
        versions.push(
            admission
                .finish()
                .into_indexed(&definition, BigramAlphabet::Ascii, inference, 1, &pool)
                .unwrap(),
        );
    }
    std::fs::remove_dir_all(&root).unwrap();
    let old = crate::inventory::ActiveRecords::all(2)
        .unwrap()
        .retired(&[RecordSlot(0), RecordSlot(1)])
        .unwrap();
    let replaced = crate::inventory::ActiveRecords::all(1)
        .unwrap()
        .retired(&[RecordSlot(0)])
        .unwrap();
    let zero = crate::inventory::ActiveRecords::all(0).unwrap();
    let compacted = [
        versions[0].compact_active(&old, 2, |_| false).unwrap(),
        versions[1].compact_active(&replaced, 1, |_| false).unwrap(),
        versions[2].compact_active(&zero, 0, |_| true).unwrap(),
    ];
    assert!(compacted[0].index.files.is_empty());
    assert!(compacted[1].index.files.is_empty());
    assert_eq!(compacted[2].index.files.len(), 1);
    assert!(compacted.iter().all(|part| part.index.records.is_empty()));
    let refs = compacted.iter().map(|part| &part.index).collect::<Vec<_>>();
    let combined = IndexedCollectionSegment::consolidate(&refs, 0).unwrap();
    assert_eq!(combined.index.files.len(), 1);
    assert_eq!(combined.index.files[0].entry.id, FileId(77));
    assert_eq!(
        combined.index.files[0].entry.revision,
        versions[2].files[0].entry.revision
    );
    assert!(combined.index.records.is_empty());
    let deleted = versions[2].compact_active(&zero, 0, |_| false).unwrap();
    assert!(deleted.index.files.is_empty());
    assert!(deleted.index.records.is_empty());
    assert!(versions[0].compact_active(&old, 1, |_| false).is_err());
}

#[test]
fn compaction_remaps_retained_fallback_diagnostics() {
    let root =
        std::env::temp_dir().join(format!("objekte-segment-fallback-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("broken.rows");
    std::fs::write(&path, b"(bad)\n(worse)\n").unwrap();
    let definition =
        CollectionDefinition::folder("data", ".", crate::source::test_support::line_format());
    let source = crate::source::test_support::fixture_registry()
        .resolve(&crate::source::test_support::line_format())
        .unwrap();
    let binding = FilePreparationBinding {
        factory: crate::index::storage::test_factory(),
        collection: CollectionId(0),
        definition: &definition,
        source: &source,
    };
    let input = SourceFileInput {
        id: FileId(66),
        relative_path: "broken.rows".into(),
        revision: SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap(),
        created_ns: None,
    };
    let inference = InferenceConfig {
        mode: InferenceMode::Disabled,
        policy: InferencePolicy::default(),
    };
    let prepared = prepare_file_build(
        &root,
        input.clone(),
        &[binding],
        BigramAlphabet::Ascii,
        inference,
        1,
    )
    .unwrap();
    let mut admission = SegmentAdmission::new(CollectionId(0), &source);
    admission
        .admit(&input, prepared.collections.into_iter().next().unwrap())
        .unwrap();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let indexed = admission
        .finish()
        .into_indexed(&definition, BigramAlphabet::Ascii, inference, 1, &pool)
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
    let active = crate::inventory::ActiveRecords::all(2)
        .unwrap()
        .retired(&[RecordSlot(0)])
        .unwrap();
    let compacted = indexed.compact_active(&active, 2, |_| true).unwrap();
    assert_eq!(compacted.old_to_new, [None, Some(RecordSlot(0))]);
    assert_eq!(compacted.index.records.len(), 1);
    assert_eq!(compacted.index.records[0].diagnostic_ref, DiagnosticRef(2));
    assert_eq!(compacted.index.files[0].entry.diagnostics.len(), 2);
    assert_eq!(compacted.index.files[0].entry.diagnostics[0].record, None);
    assert_eq!(
        compacted.index.files[0].entry.diagnostics[1].record,
        Some(RecordSlot(0))
    );
    assert_eq!(compacted.index.files[0].entry.status, FileStatus::Broken);
    assert_eq!(
        compacted
            .index
            .fallback_text
            .candidates("worse")
            .unwrap()
            .iter()
            .collect::<Vec<_>>(),
        [0]
    );
}

use crate::source::test_support::fixture_value;
