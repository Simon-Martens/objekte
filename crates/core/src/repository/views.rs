//! Scalar view planning, indexed page reads, and direct execution.

use super::*;


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
        let source = Source::open_scoped(&c.path, c.recursive)?;
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
