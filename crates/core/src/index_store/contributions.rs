//! Prepared contributions and exact old/new differences.
//!
//! Extraction produces [`PreparedRecordContributions`] once per record. The
//! coordinator assigns stored binding identifiers and turns the prepared form
//! into [`StoredContributions`]. Updates compute exact set differences so an
//! unchanged value or gram is never rewritten.

use std::collections::BTreeMap;

use crate::model::Number;
use crate::posting::ScalarKey;

use super::value_codec::{
    BindingId, BindingIdentity, CoverageKind, DomainKind, GramKind, GramProjection,
    RelationOccurrence, SortProjection, StoredContributions,
};

/// Extractor output for one record, before stored binding IDs are assigned.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PreparedRecordContributions {
    pub values: Vec<(BindingIdentity, ScalarKey)>,
    pub lengths: Vec<(BindingIdentity, Number)>,
    pub state_masks: Vec<(BindingIdentity, u16)>,
    pub coverage: Vec<(BindingIdentity, CoverageKind)>,
    pub domains: Vec<DomainKind>,
    pub sorts: Vec<SortProjection>,
    pub relations: Vec<RelationOccurrence>,
    pub grams: Vec<(BindingIdentity, GramKind, Vec<u64>)>,
}

impl PreparedRecordContributions {
    pub fn normalize(&mut self) {
        self.values.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        self.values.dedup();
        self.lengths.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        self.lengths.dedup();
        self.state_masks.sort_by(|a, b| a.0.cmp(&b.0));
        self.state_masks.dedup_by(|a, b| a.0 == b.0);
        self.coverage
            .sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.tag().cmp(&b.1.tag())));
        self.coverage.dedup();
        self.domains.sort_by_key(|domain| domain.tag());
        self.domains.dedup();
        self.sorts.sort_by_key(|sort| sort.definition);
        self.sorts.dedup_by_key(|sort| sort.definition);
        self.relations.sort_by(|a, b| {
            a.declaration_ordinal
                .cmp(&b.declaration_ordinal)
                .then_with(|| a.occurrence_ordinal.cmp(&b.occurrence_ordinal))
        });
        self.grams.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| a.1.tag().cmp(&b.1.tag()))
        });
        for (_, _, grams) in &mut self.grams {
            grams.sort_unstable();
            grams.dedup();
        }
        self.grams
            .dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
    }
}

/// Assign stored binding IDs to prepared contributions.
pub fn bind_prepared(
    prepared: &PreparedRecordContributions,
    record: super::value_codec::RecordMetadata,
    mut resolve: impl FnMut(&BindingIdentity) -> BindingId,
) -> StoredContributions {
    let mut values = prepared
        .values
        .iter()
        .map(|(identity, value)| (resolve(identity), value.clone()))
        .collect::<Vec<_>>();
    let mut lengths = prepared
        .lengths
        .iter()
        .map(|(identity, length)| (resolve(identity), length.clone()))
        .collect::<Vec<_>>();
    let mut state_masks = prepared
        .state_masks
        .iter()
        .map(|(identity, mask)| (resolve(identity), *mask))
        .collect::<Vec<_>>();
    let mut coverage = prepared
        .coverage
        .iter()
        .map(|(identity, kind)| (resolve(identity), *kind))
        .collect::<Vec<_>>();
    let mut grams: BTreeMap<(BindingId, u8), Vec<u64>> = BTreeMap::new();
    for (identity, kind, list) in &prepared.grams {
        let entry = grams.entry((resolve(identity), kind.tag())).or_default();
        entry.extend_from_slice(list);
        entry.sort_unstable();
        entry.dedup();
    }
    values.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    values.dedup();
    lengths.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    lengths.dedup();
    state_masks.sort_by(|a, b| a.0.cmp(&b.0));
    state_masks.dedup_by(|a, b| a.0 == b.0);
    coverage.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.tag().cmp(&b.1.tag())));
    coverage.dedup();
    let mut domains = prepared.domains.clone();
    domains.sort_by_key(|domain| domain.tag());
    domains.dedup();
    let mut sorts = prepared.sorts.clone();
    sorts.sort_by_key(|sort| sort.definition);
    sorts.dedup_by_key(|sort| sort.definition);
    let grams = grams
        .into_iter()
        .map(|((binding, kind), grams)| GramProjection {
            binding,
            kind: if kind == super::tables::GRAM_SKIP {
                GramKind::Skip
            } else {
                GramKind::Normal
            },
            grams,
        })
        .collect();
    StoredContributions {
        record,
        values,
        lengths,
        state_masks,
        coverage,
        domains,
        sorts,
        relations: prepared.relations.clone(),
        grams,
    }
}

/// Exact differences between two stored contributions for a retained slot.
#[derive(Clone, Debug, Default)]
pub struct ContributionDiff {
    pub values_removed: Vec<(BindingId, ScalarKey)>,
    pub values_added: Vec<(BindingId, ScalarKey)>,
    pub lengths_removed: Vec<(BindingId, Number)>,
    pub lengths_added: Vec<(BindingId, Number)>,
    pub coverage_removed: Vec<(BindingId, CoverageKind)>,
    pub coverage_added: Vec<(BindingId, CoverageKind)>,
    pub domains_removed: Vec<DomainKind>,
    pub domains_added: Vec<DomainKind>,
    pub sorts_removed: Vec<SortProjection>,
    pub sorts_added: Vec<SortProjection>,
    pub relations_removed: Vec<RelationOccurrence>,
    pub relations_added: Vec<RelationOccurrence>,
    pub grams_removed: Vec<GramProjection>,
    pub grams_added: Vec<GramProjection>,
}

impl ContributionDiff {
    pub fn is_empty(&self) -> bool {
        self.values_removed.is_empty()
            && self.values_added.is_empty()
            && self.lengths_removed.is_empty()
            && self.lengths_added.is_empty()
            && self.coverage_removed.is_empty()
            && self.coverage_added.is_empty()
            && self.domains_removed.is_empty()
            && self.domains_added.is_empty()
            && self.sorts_removed.is_empty()
            && self.sorts_added.is_empty()
            && self.relations_removed.is_empty()
            && self.relations_added.is_empty()
            && self.grams_removed.is_empty()
            && self.grams_added.is_empty()
    }
}

pub fn diff(old: &StoredContributions, new: &StoredContributions) -> ContributionDiff {
    let mut diff = ContributionDiff {
        values_removed: difference(&old.values, &new.values),
        values_added: difference(&new.values, &old.values),
        lengths_removed: difference(&old.lengths, &new.lengths),
        lengths_added: difference(&new.lengths, &old.lengths),
        coverage_removed: difference_by_tag(&old.coverage, &new.coverage),
        coverage_added: difference_by_tag(&new.coverage, &old.coverage),
        domains_removed: difference_by_key(&old.domains, &new.domains, |domain| domain.tag()),
        domains_added: difference_by_key(&new.domains, &old.domains, |domain| domain.tag()),
        sorts_removed: difference_by_key(&old.sorts, &new.sorts, |projection| {
            projection.definition
        }),
        sorts_added: difference_by_key(&new.sorts, &old.sorts, |projection| {
            projection.definition
        }),
        relations_removed: difference(&old.relations, &new.relations),
        relations_added: difference(&new.relations, &old.relations),
        ..Default::default()
    };
    let old_grams = gram_map(&old.grams);
    let new_grams = gram_map(&new.grams);
    for (key, grams) in &old_grams {
        let removed = match new_grams.get(key) {
            Some(new_grams) => grams
                .iter()
                .filter(|gram| !new_grams.contains(*gram))
                .copied()
                .collect::<Vec<_>>(),
            None => grams.iter().copied().collect(),
        };
        if !removed.is_empty() {
            diff.grams_removed.push(GramProjection {
                binding: key.0,
                kind: key.1,
                grams: removed,
            });
        }
    }
    for (key, grams) in &new_grams {
        let added = match old_grams.get(key) {
            Some(old_grams) => grams
                .iter()
                .filter(|gram| !old_grams.contains(*gram))
                .copied()
                .collect::<Vec<_>>(),
            None => grams.iter().copied().collect(),
        };
        if !added.is_empty() {
            diff.grams_added.push(GramProjection {
                binding: key.0,
                kind: key.1,
                grams: added,
            });
        }
    }
    diff
}

fn gram_map(
    grams: &[GramProjection],
) -> BTreeMap<(BindingId, GramKind), std::collections::BTreeSet<u64>> {
    let mut map: BTreeMap<(BindingId, GramKind), std::collections::BTreeSet<u64>> =
        BTreeMap::new();
    for projection in grams {
        map.entry((projection.binding, projection.kind))
            .or_default()
            .extend(projection.grams.iter().copied());
    }
    map
}

fn difference<T: Clone + PartialEq>(left: &[T], right: &[T]) -> Vec<T> {
    left.iter()
        .filter(|item| !right.contains(item))
        .cloned()
        .collect()
}

fn difference_by_tag<T: Clone + Tagged>(left: &[T], right: &[T]) -> Vec<T> {
    left.iter()
        .filter(|item| {
            !right
                .iter()
                .any(|other| item.key() == other.key() && item.tag() == other.tag())
        })
        .cloned()
        .collect()
}

fn difference_by_key<T: Clone, K: PartialEq>(
    left: &[T],
    right: &[T],
    key: impl Fn(&T) -> K,
) -> Vec<T> {
    left.iter()
        .filter(|item| !right.iter().any(|other| key(item) == key(other)))
        .cloned()
        .collect()
}

trait Tagged {
    type Key: PartialEq;
    fn key(&self) -> Self::Key;
    fn tag(&self) -> u8;
}

impl Tagged for (BindingId, CoverageKind) {
    type Key = BindingId;
    fn key(&self) -> Self::Key {
        self.0
    }
    fn tag(&self) -> u8 {
        self.1.tag()
    }
}
