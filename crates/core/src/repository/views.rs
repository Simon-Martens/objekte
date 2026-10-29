//! Scalar view planning, indexed page reads, and direct execution.

use super::*;

type ScannedView = (
    Vec<crate::ViewRow>,
    BTreeMap<String, RecordState>,
    Option<usize>,
    String,
);


impl Repository {
    pub fn open_view(&mut self, collection: &str, request: &ViewRequest) -> Result<ViewResult> {
        let before = self.metrics.clone();
        self.reload()?;
        let c = self.collection(collection)?;
        let plan = Plan::compile(&c, request)?;
        let mut result = match self.options.cache {
            CacheMode::Disabled => self.direct(&c, Some(&plan), false)?.unwrap(),
            CacheMode::Memory => self.view_memory(&c, &plan)?,
            CacheMode::Segments => self.view_segments(&c, &plan)?,
        };
        result.metrics = self.metrics.since(&before);
        Ok(result)
    }

    fn view_memory(&mut self, c: &Collection, plan: &Plan) -> Result<ViewResult> {
        use crate::Direction;
        self.ensure_picker(c)?;
        let picker = self.file_pickers.get(&c.name).expect("picker exists");
        let mut bits: Option<Vec<u64>> = None;
        let mut all_filters_indexed = true;
        for filter in &plan.filters {
            if let Some(next) =
                picker.view_filter_bits(&filter.field, filter.operator, &filter.value)
            {
                if let Some(current) = &mut bits {
                    for (word, next) in current.iter_mut().zip(next) {
                        *word &= next;
                    }
                } else {
                    bits = Some(next);
                }
            } else {
                all_filters_indexed = false;
            }
        }
        let first = &plan.sorts[0];
        let order = if first.field == "@id" && plan.sorts.len() == 1 {
            let mut order = picker.sorted_slots().to_vec();
            if first.direction == Direction::Desc {
                order.reverse();
            }
            Some(order)
        } else if plan.sorts.len() == 2
            && plan.sorts[1].field == "@id"
            && plan.sorts[1].direction == Direction::Asc
        {
            picker.view_ordered_slots(&first.field, first.direction)
        } else {
            None
        };
        let included = |slot: usize| {
            bits.as_ref()
                .is_none_or(|bits| bits[slot / 64] & (1 << (slot % 64)) != 0)
        };
        let indexed = all_filters_indexed && order.is_some();
        let (entries, total): (Vec<(String, Stamp)>, Option<usize>) = if indexed {
            let order = order.expect("indexed order");
            let total = plan
                .count
                .then(|| order.iter().filter(|&&slot| included(slot)).count());
            let entries = order
                .into_iter()
                .filter(|&slot| included(slot))
                .skip(plan.offset)
                .take(plan.limit)
                .map(|slot| {
                    let (id, stamp) = picker.view_record(slot);
                    (id.to_owned(), stamp.clone())
                })
                .collect();
            (entries, total)
        } else {
            (
                picker
                    .sorted_slots()
                    .iter()
                    .copied()
                    .filter(|&slot| included(slot))
                    .map(|slot| {
                        let (id, stamp) = picker.view_record(slot);
                        (id.to_owned(), stamp.clone())
                    })
                    .collect(),
                None,
            )
        };
        let source = Source::open(&c.path)?;
        let started = Instant::now();
        let (rows, states, total, query_plan) = if indexed {
            let mut rows = Vec::new();
            let mut states = BTreeMap::new();
            for (id, stamp) in entries {
                let raw = self.raw_from(&source, &id, Some(&stamp))?;
                let doc = self.parse(c, &id, &raw.json, true);
                rows.push(plan.row(c, &id, self.path(c, &id), &doc.data));
                states.insert(id, doc.state);
            }
            (
                rows,
                states,
                total,
                "ordered in-memory index; page source read".into(),
            )
        } else {
            let mut matches = Vec::new();
            for (id, stamp) in entries {
                let raw = self.raw_from(&source, &id, Some(&stamp))?;
                let doc = self.parse(c, &id, &raw.json, false);
                if plan.matches(&id, &doc.data) {
                    matches.push((plan.keys(&id, &doc.data), id, doc));
                }
            }
            matches.sort_by(|a, b| plan.compare_keys(&a.0, &b.0));
            let total = plan.count.then_some(matches.len());
            let mut rows = Vec::new();
            let mut states = BTreeMap::new();
            for (_, id, mut doc) in matches.into_iter().skip(plan.offset).take(plan.limit) {
                doc.validate(c);
                rows.push(plan.row(c, &id, self.path(c, &id), &doc.data));
                states.insert(id, doc.state);
            }
            (
                rows,
                states,
                total,
                "in-memory candidate scan and sort".into(),
            )
        };
        source.finish()?;
        self.metrics.query_nanos += nanos(started);
        self.memory_coverage
            .insert(c.name.clone(), "current".into());
        Ok(ViewResult {
            columns: plan.columns.clone(),
            rows,
            states,
            total,
            metrics: Metrics::default(),
            query_plan: vec![query_plan],
            view_hashes: plan.hashes.clone(),
        })
    }

    fn view_segments(&mut self, c: &Collection, plan: &Plan) -> Result<ViewResult> {
        use crate::{Direction, query::scalar_value};
        self.sync_segments(c, false)?;
        let snapshot = self
            .segment_snapshot(c)?
            .ok_or_else(|| Error::Cache("segment snapshot disappeared".into()))?;
        let mut candidates = snapshot
            .entries()
            .map(|(id, _)| id.clone())
            .collect::<BTreeSet<_>>();
        let mut all_filters_indexed = true;
        for filter in &plan.filters {
            if let Some(ids) = snapshot.indexed_filter_ids(
                &filter.field,
                filter.operator,
                &scalar_value(Some(&filter.value)),
            ) {
                candidates = candidates.intersection(&ids).cloned().collect();
            } else {
                all_filters_indexed = false;
            }
        }
        let first = &plan.sorts[0];
        let indexed_order = if first.field == "@id" && plan.sorts.len() == 1 {
            let mut ids = candidates.iter().cloned().collect::<Vec<_>>();
            if first.direction == Direction::Desc {
                ids.reverse();
            }
            Some(ids)
        } else if plan.sorts.len() == 2
            && plan.sorts[1].field == "@id"
            && plan.sorts[1].direction == Direction::Asc
        {
            snapshot.ordered_ids(&first.field, first.direction == Direction::Desc)
        } else {
            None
        };
        let source = Source::open(&c.path)?;
        let t = Instant::now();
        let (rows, states, total, query_plan) = if all_filters_indexed {
            if let Some(order) = indexed_order {
                let total = plan.count.then_some(candidates.len());
                let mut rows = Vec::new();
                let mut states = BTreeMap::new();
                for id in order
                    .into_iter()
                    .filter(|id| candidates.contains(id))
                    .skip(plan.offset)
                    .take(plan.limit)
                {
                    let entry = snapshot
                        .entry(&id)
                        .ok_or_else(|| Error::Cache(format!("segment ID {id} missing")))?;
                    let raw = self.raw_from(&source, &id, Some(&entry.stamp))?;
                    let doc = self.parse(c, &id, &raw.json, true);
                    rows.push(plan.row(c, &id, self.path(c, &id), &doc.data));
                    states.insert(id, doc.state);
                }
                (
                    rows,
                    states,
                    total,
                    "ordered segment index; page source read".to_owned(),
                )
            } else {
                self.view_segments_fallback(c, plan, &snapshot, &source, &candidates)?
            }
        } else {
            self.view_segments_fallback(c, plan, &snapshot, &source, &candidates)?
        };
        source.finish()?;
        self.metrics.query_nanos += nanos(t);
        Ok(ViewResult {
            columns: plan.columns.clone(),
            rows,
            states,
            total,
            metrics: Metrics::default(),
            query_plan: vec![query_plan],
            view_hashes: plan.hashes.clone(),
        })
    }

    fn view_segments_fallback(
        &mut self,
        c: &Collection,
        plan: &Plan,
        snapshot: &segment_cache::Snapshot,
        source: &Source,
        candidates: &BTreeSet<String>,
    ) -> Result<ScannedView> {
        let mut matches = Vec::new();
        for id in candidates {
            let entry = snapshot
                .entry(id)
                .ok_or_else(|| Error::Cache(format!("segment ID {id} missing")))?;
            let raw = self.raw_from(source, id, Some(&entry.stamp))?;
            let doc = self.parse(c, id, &raw.json, false);
            if plan.matches(id, &doc.data) {
                matches.push((plan.keys(id, &doc.data), id.clone(), doc));
            }
        }
        matches.sort_by(|a, b| plan.compare_keys(&a.0, &b.0));
        let total = plan.count.then_some(matches.len());
        let mut rows = Vec::new();
        let mut states = BTreeMap::new();
        for (_, id, mut doc) in matches.into_iter().skip(plan.offset).take(plan.limit) {
            doc.validate(c);
            rows.push(plan.row(c, &id, self.path(c, &id), &doc.data));
            states.insert(id, doc.state);
        }
        Ok((
            rows,
            states,
            total,
            "segment candidate scan and in-memory sort".into(),
        ))
    }

    pub(super) fn direct(
        &mut self,
        c: &Collection,
        plan: Option<&Plan>,
        validate: bool,
    ) -> Result<Option<ViewResult>> {
        self.memory_coverage.insert(c.name.clone(), "dirty".into());
        let (source, files) = self.inventory(c)?;
        let mut seen = BTreeMap::new();
        let mut rows = Vec::new();
        let mut states = BTreeMap::new();
        let mut edges = BTreeMap::new();
        for id in files.keys() {
            let raw = self.raw_from(&source, id, files.get(id))?;
            let data = self.parse(c, id, &raw.json, validate);
            edges.insert((c.name.clone(), id.clone()), self.extract(c, id, &data)?);
            if let Some(plan) = plan {
                let t = Instant::now();
                if plan.matches(id, &data) {
                    states.insert(id.clone(), data.state.clone());
                    rows.push((
                        plan.keys(id, &data),
                        plan.row(c, id, self.path(c, id), &data),
                    ));
                }
                self.metrics.query_nanos += nanos(t);
            }
            seen.insert(id.clone(), raw.stamp);
        }
        source.finish()?;
        if self.inventory(c)?.1 != seen {
            return Err(Error::Unstable(c.path.clone()));
        }
        self.memory_edges.retain(|(name, _), _| name != &c.name);
        self.memory_edges.extend(edges);
        self.memory_coverage
            .insert(c.name.clone(), "current".into());
        if let Some(plan) = plan {
            let t = Instant::now();
            rows.sort_by(|a, b| plan.compare(a, b));
            let total = plan.count.then_some(rows.len());
            let rows: Vec<_> = rows
                .into_iter()
                .skip(plan.offset)
                .take(plan.limit)
                .map(|(_, r)| r)
                .collect();
            self.metrics.query_nanos += nanos(t);
            Ok(Some(ViewResult {
                columns: plan.columns.clone(),
                states: rows
                    .iter()
                    .map(|r| (r.id.clone(), states.remove(&r.id).unwrap()))
                    .collect(),
                rows,
                total,
                metrics: Metrics::default(),
                query_plan: vec!["direct source scan".into()],
                view_hashes: plan.hashes.clone(),
            }))
        } else {
            Ok(None)
        }
    }
}
