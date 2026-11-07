//! Bounded construction-only field observation and posting admission.

use std::collections::{BTreeMap, HashMap};

use roaring::RoaringBitmap;

use crate::inventory::append_shifted_bitmap;
use crate::model::{LogicalType, Number, RecordSlot, Value};
use crate::posting::{PostingIndex, PostingStats, ScalarKey};
use crate::reference::Schema;

use super::{
    InferenceConfig, InferenceIndexStatus, InferenceMode, InferencePolicy, InferenceReport,
    InferenceState, InferredFieldReport, ObservedStep, typed_path,
};


/// Observations retained for one path while construction batches are admitted.
struct FieldState {
    path: Vec<String>,
    kinds: Vec<LogicalType>,
    present: RoaringBitmap,
    null_occurrence: RoaringBitmap,
    exact_null: RoaringBitmap,
    empty_array: RoaringBitmap,
    empty_selection: RoaringBitmap,
    nonblank_records: RoaringBitmap,
    scalar_records: RoaringBitmap,
    array_records: RoaringBitmap,
    object_records: RoaringBitmap,
    through_array_records: RoaringBitmap,
    direct_records: RoaringBitmap,
    array_nesting: u8,
    fractional_number: bool,
    value_occurrences: u64,
    nonblank_occurrences: u64,
    distinct_nonblank: Option<std::collections::HashSet<String>>,
    values: Option<BTreeMap<ScalarKey, RoaringBitmap>>,
    lengths: Option<BTreeMap<Number, RoaringBitmap>>,
    dropped: bool,
    #[cfg(test)]
    key_builds: usize,
}


impl FieldState {
    fn new(path: Vec<String>, mode: InferenceMode) -> Self {
        Self {
            path,
            kinds: Vec::new(),
            present: RoaringBitmap::new(),
            null_occurrence: RoaringBitmap::new(),
            exact_null: RoaringBitmap::new(),
            empty_array: RoaringBitmap::new(),
            empty_selection: RoaringBitmap::new(),
            nonblank_records: RoaringBitmap::new(),
            scalar_records: RoaringBitmap::new(),
            array_records: RoaringBitmap::new(),
            object_records: RoaringBitmap::new(),
            through_array_records: RoaringBitmap::new(),
            direct_records: RoaringBitmap::new(),
            array_nesting: 0,
            fractional_number: false,
            value_occurrences: 0,
            nonblank_occurrences: 0,
            distinct_nonblank: Some(Default::default()),
            values: (mode == InferenceMode::AutoIndex).then(BTreeMap::new),
            lengths: (mode == InferenceMode::AutoIndex).then(BTreeMap::new),
            dropped: false,
            #[cfg(test)]
            key_builds: 0,
        }
    }

    fn kind(&mut self, kind: LogicalType) {
        if !self.kinds.contains(&kind) {
            self.kinds.push(kind);
        }
    }

    fn drop_values(&mut self) {
        self.values = None;
        self.lengths = None;
        self.dropped = true;
    }

    fn add_scalar(&mut self, slot: RecordSlot, value: &Value, policy: InferencePolicy) -> usize {
        let mut retained = 8;
        let kind = match value {
            Value::Bool(_) => LogicalType::Bool,
            Value::Number(Number::Integer(_)) => LogicalType::Number,
            Value::Number(Number::Float(_)) => {
                self.fractional_number = true;
                LogicalType::Number
            }
            Value::Number(Number::ExactDecimal(_)) => {
                self.value_occurrences = self.value_occurrences.saturating_add(1);
                self.kind(LogicalType::Number);
                self.drop_values();
                return retained;
            }
            Value::String(_) => LogicalType::String,
            Value::Id(_) => LogicalType::Id,
            Value::Date(_) => LogicalType::Date,
            Value::Datetime(_) => LogicalType::Datetime,
            Value::Unixtime(_) => LogicalType::Unixtime,
            Value::Null => {
                self.null_occurrence.insert(slot.0);
                return retained;
            }
            _ => return retained,
        };
        self.value_occurrences = self.value_occurrences.saturating_add(1);
        self.kind(kind);
        if let Value::String(value) = value
            && !value.trim().is_empty()
        {
            self.nonblank_occurrences = self.nonblank_occurrences.saturating_add(1);
            self.nonblank_records.insert(slot.0);
            if let Some(distinct) = &mut self.distinct_nonblank {
                if value.len() > policy.max_string_bytes
                    || distinct.len() >= policy.max_distinct_nonblank_strings
                        && !distinct.contains(value)
                    || distinct.iter().map(String::len).sum::<usize>() + value.len()
                        > policy.max_distinct_string_bytes
                {
                    self.distinct_nonblank = None;
                    self.drop_values();
                } else if distinct.insert(value.clone()) {
                    retained += value.len() + 48;
                }
            }
        }
        // A dropped provisional dictionary must not keep cloning later values.
        // Check the string bound on the borrowed value before making an owned key.
        if self.values.is_none() {
            return retained;
        }
        match value {
            Value::String(text) if text.len() > policy.max_string_bytes => {
                self.drop_values();
                return retained;
            }
            Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
            // These kinds cannot be admitted as inferred typed postings.
            _ => return retained,
        }

        #[cfg(test)]
        {
            self.key_builds += 1;
        }
        let key = ScalarKey::from_value(value).expect("eligible inferred scalar");
        let values = self
            .values
            .as_mut()
            .expect("checked provisional dictionary");
        if values.len() >= policy.max_distinct_values_per_path && !values.contains_key(&key) {
            self.drop_values();
        } else {
            if !values.contains_key(&key) {
                retained += 128
                    + match &key {
                        ScalarKey::String(text) => text.len(),
                        _ => 0,
                    };
            }
            values.entry(key).or_default().insert(slot.0);
        }
        retained
    }

    fn merge_shifted(
        &mut self,
        other: Self,
        base: u32,
        policy: InferencePolicy,
    ) -> Result<(), String> {
        for kind in other.kinds {
            self.kind(kind);
        }
        for (target, source) in [
            (&mut self.present, &other.present),
            (&mut self.null_occurrence, &other.null_occurrence),
            (&mut self.exact_null, &other.exact_null),
            (&mut self.empty_array, &other.empty_array),
            (&mut self.empty_selection, &other.empty_selection),
            (&mut self.nonblank_records, &other.nonblank_records),
            (&mut self.scalar_records, &other.scalar_records),
            (&mut self.array_records, &other.array_records),
            (&mut self.object_records, &other.object_records),
            (
                &mut self.through_array_records,
                &other.through_array_records,
            ),
            (&mut self.direct_records, &other.direct_records),
        ] {
            append_shifted_bitmap(target, source, base)?;
        }
        self.array_nesting = self.array_nesting.max(other.array_nesting);
        self.fractional_number |= other.fractional_number;
        self.value_occurrences = self
            .value_occurrences
            .checked_add(other.value_occurrences)
            .ok_or("inference occurrence overflow")?;
        self.nonblank_occurrences = self
            .nonblank_occurrences
            .checked_add(other.nonblank_occurrences)
            .ok_or("inference occurrence overflow")?;
        match (&mut self.distinct_nonblank, other.distinct_nonblank) {
            (Some(existing), Some(incoming)) => {
                for value in incoming {
                    existing.insert(value);
                    if existing.len() > policy.max_distinct_nonblank_strings
                        || existing.iter().map(String::len).sum::<usize>()
                            > policy.max_distinct_string_bytes
                    {
                        self.distinct_nonblank = None;
                        break;
                    }
                }
            }
            _ => self.distinct_nonblank = None,
        }
        if self.dropped || other.dropped || self.distinct_nonblank.is_none() {
            self.drop_values();
        } else if let (Some(existing), Some(incoming)) = (&mut self.values, other.values) {
            for (key, bitmap) in incoming {
                if existing.len() >= policy.max_distinct_values_per_path
                    && !existing.contains_key(&key)
                {
                    self.drop_values();
                    break;
                }
                append_shifted_bitmap(existing.entry(key).or_default(), &bitmap, base)?;
            }
        }
        if let (Some(existing), Some(incoming)) = (&mut self.lengths, other.lengths) {
            for (key, bitmap) in incoming {
                append_shifted_bitmap(existing.entry(key).or_default(), &bitmap, base)?;
            }
        }
        Ok(())
    }
}


/// One batch or coordinator's bounded construction-only observation state.
pub(crate) struct InferenceBuilder {
    config: InferenceConfig,
    paths: HashMap<Vec<ObservedStep>, usize>,
    fields: Vec<FieldState>,
    empty_ancestors: HashMap<Vec<ObservedStep>, RoaringBitmap>,
    selected: HashMap<usize, (u32, bool)>,
    eligible: u32,
    omitted_paths: u64,
    complete: bool,
    estimated_bytes: usize,
    batch: bool,
    batch_limit: usize,
    coordinator_limit: usize,
}


/// Reused traversal state for one already parsed logical record.
struct VisitState {
    slot: RecordSlot,
    path: Vec<ObservedStep>,
    contributions: usize,
}


impl InferenceBuilder {
    pub(crate) fn new(config: InferenceConfig, batch: bool) -> Option<Self> {
        Self::with_job_count(config, batch, 1)
    }

    pub(crate) fn with_job_count(
        config: InferenceConfig,
        batch: bool,
        _max_jobs: usize,
    ) -> Option<Self> {
        // A batch's drop decision depends on its contents, not worker count.
        // The scheduler limits simultaneous batches to the repository slack.
        let batch_limit = config.policy.max_batch_bytes;
        (config.mode != InferenceMode::Disabled).then(|| Self {
            config,
            paths: HashMap::new(),
            fields: Vec::new(),
            empty_ancestors: HashMap::new(),
            selected: HashMap::new(),
            eligible: 0,
            omitted_paths: 0,
            complete: true,
            estimated_bytes: 0,
            batch,
            batch_limit,
            coordinator_limit: config.policy.max_total_bytes,
        })
    }

    pub(crate) fn for_collection(config: InferenceConfig, collections: usize) -> Option<Self> {
        let mut builder = Self::new(config, false)?;
        // All collection builders coexist until finalization. Give each the
        // same deterministic share of the coordinator's retained allowance.
        builder.coordinator_limit = config.policy.max_total_bytes / collections.max(1);
        Some(builder)
    }

    fn limit(&self) -> usize {
        if self.batch {
            self.batch_limit
        } else {
            self.coordinator_limit
        }
    }

    fn charge(&mut self, bytes: usize) {
        self.estimated_bytes = self.estimated_bytes.saturating_add(bytes);
        if self.estimated_bytes > self.limit() {
            self.complete = false;
            for field in &mut self.fields {
                field.drop_values();
            }
        }
    }

    fn field(&mut self, path: &[ObservedStep]) -> Option<usize> {
        if path.is_empty() || !self.complete {
            return None;
        }
        if let Some(index) = self.paths.get(path) {
            return Some(*index);
        }
        if path.len() > self.config.policy.max_depth
            || self.fields.len() >= self.config.policy.max_paths
        {
            self.complete = false;
            self.omitted_paths = self.omitted_paths.saturating_add(1);
            for field in &mut self.fields {
                field.drop_values();
            }
            return None;
        }
        let extra = 256
            + path
                .iter()
                .map(|step| match step {
                    ObservedStep::Field(name) => name.len(),
                    ObservedStep::ArrayIndex(_) => 4,
                })
                .sum::<usize>();
        self.charge(extra);
        if !self.complete {
            self.omitted_paths = self.omitted_paths.saturating_add(1);
            return None;
        }
        let fields = path
            .iter()
            .map(|step| match step {
                ObservedStep::Field(name) => Some(name.clone()),
                ObservedStep::ArrayIndex(_) => None,
            })
            .collect::<Option<Vec<_>>>()?;
        let index = self.fields.len();
        let owned = path.to_vec();
        self.paths.insert(owned, index);
        self.fields.push(FieldState::new(fields, self.config.mode));
        Some(index)
    }

    fn visit(
        &mut self,
        value: &Value,
        state: &mut VisitState,
        terminal: bool,
        through_array: bool,
        nesting: u8,
    ) {
        if !self.complete {
            return;
        }
        state.contributions += 1;
        if state.contributions > self.config.policy.max_record_contributions {
            self.complete = false;
            for field in &mut self.fields {
                field.drop_values();
            }
            return;
        }
        let index = self.field(&state.path);
        if let Some(index) = index {
            let field = &mut self.fields[index];
            let mut charge = 0;
            if matches!(value, Value::Array(_)) {
                field.array_nesting = field.array_nesting.max(nesting.saturating_add(1));
            }
            if terminal {
                field.present.insert(state.slot.0);
                charge += 16;
                field.array_nesting = field.array_nesting.max(nesting);
                let selection = self.selected.entry(index).or_insert((0, false));
                selection.0 = selection.0.saturating_add(1);
                selection.1 = selection.0 == 1 && matches!(value, Value::Null);
                if through_array {
                    field.through_array_records.insert(state.slot.0);
                } else {
                    field.direct_records.insert(state.slot.0);
                }
                match value {
                    Value::Array(array) => {
                        field.array_records.insert(state.slot.0);
                        if array.values.is_empty() {
                            field.empty_array.insert(state.slot.0);
                            field.empty_selection.insert(state.slot.0);
                        }
                        if let Some(lengths) = &mut field.lengths {
                            let key = Number::integer(array.values.len() as i128);
                            if !lengths.contains_key(&key) {
                                charge += 128;
                            }
                            lengths.entry(key).or_default().insert(state.slot.0);
                        }
                    }
                    Value::Object(_) => {
                        field.object_records.insert(state.slot.0);
                    }
                    _ => {
                        field.scalar_records.insert(state.slot.0);
                    }
                }
            }
            if !matches!(value, Value::Object(_) | Value::Array(_)) {
                charge += field.add_scalar(state.slot, value, self.config.policy);
            }
            self.charge(charge);
        }
        if !self.complete {
            return;
        }
        match value {
            Value::Object(map) => {
                for (name, child) in map {
                    state.path.push(ObservedStep::Field(name.clone()));
                    self.visit(child, state, true, through_array, nesting);
                    state.path.pop();
                    if !self.complete {
                        break;
                    }
                }
            }
            Value::Array(array) => {
                if array.values.is_empty() && !state.path.is_empty() {
                    self.empty_ancestors
                        .entry(state.path.clone())
                        .or_default()
                        .insert(state.slot.0);
                    self.charge(16);
                }
                for child in &array.values {
                    self.visit(child, state, false, true, nesting.saturating_add(1));
                    if !self.complete {
                        break;
                    }
                }
            }
            _ => {}
        }
    }

    pub(crate) fn observe_record(&mut self, slot: RecordSlot, value: &Value) -> Result<(), String> {
        self.eligible = self
            .eligible
            .checked_add(1)
            .ok_or("inference coverage overflow")?;
        if !self.complete {
            return Ok(());
        }
        self.selected.clear();
        let mut state = VisitState {
            slot,
            path: Vec::new(),
            contributions: 0,
        };
        self.visit(value, &mut state, false, false, 0);
        for (index, (count, sole_null)) in self.selected.drain() {
            if count == 1 && sole_null {
                self.fields[index].exact_null.insert(slot.0);
            }
        }
        Ok(())
    }

    pub(crate) fn merge_shifted(&mut self, other: Self, base: u32) -> Result<(), String> {
        if self.config != other.config || self.batch || !other.batch {
            return Err("inference batch policy mismatch".into());
        }
        self.eligible = self
            .eligible
            .checked_add(other.eligible)
            .ok_or("inference coverage overflow")?;
        self.omitted_paths = self.omitted_paths.saturating_add(other.omitted_paths);
        // Decline before merging a whole batch, while it still occupies the
        // scheduler's separate in-flight allowance.
        self.charge(other.estimated_bytes);
        self.complete &= other.complete;
        if !self.complete {
            for field in &mut self.fields {
                field.drop_values();
            }
            return Ok(());
        }
        for (path, bitmap) in other.empty_ancestors {
            append_shifted_bitmap(self.empty_ancestors.entry(path).or_default(), &bitmap, base)?;
        }
        for incoming in other.fields {
            let typed = typed_path(&incoming.path);
            if let Some(index) = self.paths.get(&typed).copied() {
                self.fields[index].merge_shifted(incoming, base, self.config.policy)?;
            } else if let Some(index) = self.field(&typed) {
                let mut new = FieldState::new(incoming.path.clone(), self.config.mode);
                new.merge_shifted(incoming, base, self.config.policy)?;
                self.fields[index] = new;
            }
        }
        Ok(())
    }

    pub(crate) fn finish(
        mut self,
        logical: &RoaringBitmap,
        record_count: usize,
        schema: &Schema,
    ) -> Result<InferenceState, String> {
        if self.eligible as u64 != logical.len() {
            return Err("inference logical coverage mismatch".into());
        }
        let mut report_fields = Vec::with_capacity(self.fields.len());
        let mut indexes = HashMap::new();
        let mut empty_selections = HashMap::new();
        self.fields.sort_by(|a, b| a.path.cmp(&b.path));
        for mut field in self.fields {
            for depth in 1..field.path.len() {
                if let Some(empty) = self.empty_ancestors.get(&typed_path(&field.path[..depth])) {
                    field.present |= empty;
                    field.empty_selection |= empty;
                }
            }
            if !field.present.is_subset(logical) || !field.exact_null.is_subset(&field.present) {
                return Err("inference path membership outside logical domain".into());
            }
            field.kinds.sort_by_key(|kind| format!("{kind:?}"));
            let declared = schema.fields.get(&field.path);
            let scalar_kind = if field.kinds.len() == 1 {
                Some(field.kinds[0].clone())
            } else {
                None
            };
            let mixed_shape = (!field.scalar_records.is_empty() && !field.array_records.is_empty())
                || (!field.direct_records.is_empty() && !field.through_array_records.is_empty())
                || !field.object_records.is_empty();
            let string_admissible = if scalar_kind == Some(LogicalType::String) {
                let distinct = field
                    .distinct_nonblank
                    .as_ref()
                    .map(std::collections::HashSet::len);
                distinct.is_some_and(|distinct| {
                    let policy = self.config.policy;
                    field.nonblank_occurrences >= policy.min_nonblank_occurrences
                        && field.nonblank_records.len() >= policy.min_nonblank_records
                        && distinct <= policy.max_distinct_nonblank_strings
                        && distinct <= (field.nonblank_occurrences / 10).max(4) as usize
                        && field.nonblank_occurrences.saturating_sub(distinct as u64) * 100
                            >= field.nonblank_occurrences * policy.minimum_repeated_percent as u64
                })
            } else {
                true
            };
            let (status, reason) = if !self.complete {
                (
                    InferenceIndexStatus::Incomplete,
                    "observation budget exceeded",
                )
            } else if self.config.mode == InferenceMode::ReportOnly {
                (InferenceIndexStatus::NotRequested, "report-only mode")
            } else if declared.is_some_and(|field| field.indexed) {
                (
                    InferenceIndexStatus::Redundant,
                    "declared posting already exists",
                )
            } else if declared.is_some() {
                (
                    InferenceIndexStatus::ExplicitOptOut,
                    "declared field disabled indexing",
                )
            } else if field.dropped || field.values.is_none() {
                (
                    InferenceIndexStatus::BudgetExceeded,
                    "provisional values exceeded a bound",
                )
            } else if mixed_shape || scalar_kind.is_none() {
                (
                    InferenceIndexStatus::Incompatible,
                    "mixed or unknown observed shape/kind",
                )
            } else if !matches!(
                scalar_kind,
                Some(LogicalType::Bool | LogicalType::Number | LogicalType::String)
            ) {
                (
                    InferenceIndexStatus::Incompatible,
                    "kind is not an inferred posting target",
                )
            } else if !string_admissible {
                (
                    InferenceIndexStatus::BelowThreshold,
                    "string repetition threshold not met",
                )
            } else {
                (
                    InferenceIndexStatus::Built,
                    "complete compatible logical coverage",
                )
            };
            let mut index_bytes = 0usize;
            if status == InferenceIndexStatus::Built {
                let kind = scalar_kind.unwrap();
                let mut index = PostingIndex {
                    path: field.path.clone(),
                    kind,
                    multi: !field.array_records.is_empty()
                        || !field.through_array_records.is_empty(),
                    presence: field.present.clone(),
                    nulls: field.exact_null.clone(),
                    values: field.values.take().unwrap_or_default(),
                    lengths: field.lengths.take().unwrap_or_default(),
                    covered_records: self.eligible,
                    stats: PostingStats::default(),
                };
                index.finish_stats();
                index.validate_eligible(record_count, logical.len() as usize)?;
                index.validate_domain(logical)?;
                index_bytes = index
                    .values
                    .iter()
                    .map(|(key, slots)| {
                        128 + match key {
                            ScalarKey::String(text) => text.len(),
                            _ => 0,
                        } + slots.serialized_size()
                    })
                    .sum::<usize>()
                    + index
                        .lengths
                        .values()
                        .map(|slots| 64 + slots.serialized_size())
                        .sum::<usize>();
                empty_selections.insert(field.path.clone(), field.empty_selection.clone());
                indexes.insert(field.path.clone(), index);
            }
            let summary_bytes_estimate = 256
                + field.path.iter().map(String::len).sum::<usize>()
                + field.present.serialized_size()
                + field.null_occurrence.serialized_size()
                + field.empty_array.serialized_size();
            let retained_bytes_estimate = summary_bytes_estimate.saturating_add(index_bytes);
            let integer_only = field
                .kinds
                .contains(&LogicalType::Number)
                .then_some(!field.fractional_number);
            report_fields.push(InferredFieldReport {
                path: field.path,
                observed_kinds: field.kinds,
                scalar_records: field.scalar_records.len(),
                array_records: field.array_records.len(),
                object_records: field.object_records.len(),
                array_nesting: field.array_nesting,
                integer_only,
                eligible_records: logical.len(),
                unavailable_records: record_count as u64 - logical.len(),
                present_records: field.present.len(),
                missing_records: logical.len() - field.present.len(),
                null_occurrence_records: field.null_occurrence.len(),
                exact_null_records: field.exact_null.len(),
                empty_array_records: field.empty_array.len(),
                empty_selection_records: field.empty_selection.len(),
                value_occurrences: field.value_occurrences,
                nonblank_occurrences: field.nonblank_occurrences,
                nonblank_records: field.nonblank_records.len(),
                distinct_nonblank_strings: field
                    .distinct_nonblank
                    .as_ref()
                    .map(std::collections::HashSet::len),
                index_status: status,
                reason: reason.into(),
                complete: self.complete && field.distinct_nonblank.is_some(),
                summary_bytes_estimate,
                index_bytes_estimate: index_bytes,
                retained_bytes_estimate,
            });
        }
        let report = InferenceReport {
            mode: self.config.mode,
            policy: self.config.policy,
            eligible_records: logical.len(),
            unavailable_records: record_count as u64 - logical.len(),
            omitted_paths: self.omitted_paths,
            complete: self.complete,
            observed_paths: report_fields.len(),
            admitted_paths: indexes.len(),
            dropped_paths: report_fields
                .iter()
                .filter(|field| {
                    matches!(
                        field.index_status,
                        InferenceIndexStatus::Incompatible
                            | InferenceIndexStatus::Incomplete
                            | InferenceIndexStatus::BudgetExceeded
                            | InferenceIndexStatus::BelowThreshold
                    )
                })
                .count(),
            summary_bytes_estimate: report_fields.iter().fold(0usize, |total, field| {
                total.saturating_add(field.summary_bytes_estimate)
            }),
            index_bytes_estimate: report_fields.iter().fold(0usize, |total, field| {
                total.saturating_add(field.index_bytes_estimate)
            }),
            retained_bytes_estimate: report_fields.iter().fold(0usize, |total, field| {
                total.saturating_add(field.retained_bytes_estimate)
            }),
            fields: report_fields,
        };
        Ok(InferenceState {
            report,
            indexes,
            empty_selections,
        })
    }
}


#[cfg(test)]
mod allocation_tests {
    use super::*;

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
        assert_eq!(indexed.key_builds, 1);
    }
}
