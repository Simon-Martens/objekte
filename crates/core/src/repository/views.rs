//! Scalar view planning, indexed page reads, and direct execution.

use super::*;
use crate::query::ViewSelection;


/// Retain full JSON only when the final page still needs schema validation.
enum RetainedView {
    Unvalidated(String, Document),
    Ready(crate::ViewRow, RecordState),
}


impl Repository {
    pub fn open_view(&mut self, collection: &str, request: &ViewRequest) -> Result<ViewResult> {
        let before = self.metrics.clone();
        self.reload()?;
        let c = self.collection(collection)?;
        let plan = Plan::compile(&c, request)?;
        let mut result = match self.options.cache {
            CacheMode::Disabled => self.direct(&c, Some(&plan), false)?.unwrap(),
            CacheMode::Memory => self.view_memory(&c, &plan)?,
        };
        result.metrics = self.metrics.since(&before);
        Ok(result)
    }

    fn view_memory(&mut self, c: &Collection, plan: &Plan) -> Result<ViewResult> {
        use crate::Direction;
        self.ensure_picker(c)?;
        let picker = self.file_pickers.get(&c.name).expect("picker exists");
        let optimistic = !picker.verified();
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
            Some(picker.view_id_order(first.direction))
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
        let (entries, indexed_total): (Vec<(String, Stamp)>, Option<usize>) = if indexed {
            let order = order.expect("indexed order");
            let total = plan
                .count
                .then(|| order.clone().filter(|&slot| included(slot)).count());
            let entries = order
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
            (Vec::new(), None)
        };
        let source = Source::open_scoped(&c.path, c.recursive)?;
        let started = Instant::now();
        let (rows, states, total, query_plan) = if indexed {
            let mut rows = Vec::new();
            let mut states = BTreeMap::new();
            for (id, stamp) in entries {
                let raw = match self.raw_from(&source, &id, (!optimistic).then_some(&stamp)) {
                    Ok(raw) => raw,
                    Err(Error::Io { source: error, .. })
                        if optimistic && error.kind() == std::io::ErrorKind::NotFound =>
                    {
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                let doc = self.parse(c, &id, &raw.json, true);
                rows.push(plan.row(c, &id, self.path(c, &id), &doc.data));
                states.insert(id, doc.state);
            }
            (
                rows,
                states,
                indexed_total,
                "ordered in-memory index; page source read".into(),
            )
        } else {
            let mut selected = ViewSelection::new(plan);
            // Acquire each picker record only for the duration of one read.
            // The scan never materializes an all-files candidate vector.
            let slots = picker.sorted_slots().len();
            for position in 0..slots {
                let candidate = {
                    let picker = &self.file_pickers[&c.name];
                    let slot = picker.sorted_slots()[position];
                    included(slot).then(|| {
                        let (id, stamp) = picker.view_record(slot);
                        (id.to_owned(), stamp.clone())
                    })
                };
                let Some((id, stamp)) = candidate else {
                    continue;
                };
                let raw = match self.raw_from(&source, &id, (!optimistic).then_some(&stamp)) {
                    Ok(raw) => raw,
                    Err(Error::Io { source: error, .. })
                        if optimistic && error.kind() == std::io::ErrorKind::NotFound =>
                    {
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                let doc = self.parse(c, &id, &raw.json, false);
                if plan.matches(&id, &doc.data) {
                    selected.consider(plan.keys(&id, &doc.data), || {
                        if c.schema.is_some() && doc.state.status != RecordStatus::IncorrectJson {
                            RetainedView::Unvalidated(id, doc)
                        } else {
                            let row = plan.row(c, &id, self.path(c, &id), &doc.data);
                            RetainedView::Ready(row, doc.state)
                        }
                    });
                }
            }
            let total = selected.total();
            let mut rows = Vec::new();
            let mut states = BTreeMap::new();
            for retained in selected.into_page() {
                match retained {
                    RetainedView::Unvalidated(id, mut doc) => {
                        doc.validate(c);
                        rows.push(plan.row(c, &id, self.path(c, &id), &doc.data));
                        states.insert(id, doc.state);
                    }
                    RetainedView::Ready(row, state) => {
                        states.insert(row.id.clone(), state);
                        rows.push(row);
                    }
                }
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

    pub(super) fn direct(
        &mut self,
        c: &Collection,
        plan: Option<&Plan>,
        validate: bool,
    ) -> Result<Option<ViewResult>> {
        self.memory_coverage.insert(c.name.clone(), "dirty".into());
        let (source, files) = self.inventory(c)?;
        let mut seen = BTreeMap::new();
        let mut selected = plan.map(ViewSelection::new);
        let mut edges = BTreeMap::new();
        for id in files.keys() {
            let raw = self.raw_from(&source, id, files.get(id))?;
            let data = self.parse(c, id, &raw.json, validate);
            edges.insert((c.name.clone(), id.clone()), self.extract(c, id, &data)?);
            if let Some(plan) = plan {
                let t = Instant::now();
                if plan.matches(id, &data) {
                    selected
                        .as_mut()
                        .expect("view selection")
                        .consider(plan.keys(id, &data), || {
                            (plan.row(c, id, self.path(c, id), &data), data.state.clone())
                        });
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
            let selected = selected.expect("view selection");
            let total = selected.total();
            let page = selected.into_page();
            let states = page
                .iter()
                .map(|(row, state)| (row.id.clone(), state.clone()))
                .collect();
            let rows = page.into_iter().map(|(row, _)| row).collect();
            self.metrics.query_nanos += nanos(t);
            Ok(Some(ViewResult {
                columns: plan.columns.clone(),
                states,
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
