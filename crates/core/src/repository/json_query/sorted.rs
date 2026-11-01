//! Route sorted JSON pages to the verified index or bounded fallback.

use super::*;
use crate::sorting::{SortBuffer, SortPlan};


impl Repository {
    pub(super) fn query_json_sorted(
        &mut self,
        request: &JsonQuery,
        on_page: &mut dyn FnMut(&[Record]) -> Result<()>,
    ) -> Result<JsonQueryResult> {
        let started = Instant::now();
        let before = self.metrics.clone();
        if self.options.total_limit == 0 {
            return Err(Error::Request(
                "repository total_limit must be positive".into(),
            ));
        }
        self.reload()?;
        let names = request
            .collections
            .clone()
            .unwrap_or_else(|| self.model.collections.keys().cloned().collect::<Vec<_>>());
        let mut plans = BTreeMap::new();
        let filter = JsonFilter::compile(&request.filter, &request.parameters)?;
        let mut indexed_order = matches!(
            request.backend,
            JsonQueryBackend::Auto | JsonQueryBackend::Bigram
        ) && self.options.cache == CacheMode::Memory
            && self.options.freshness != Freshness::HashOnRequest;
        for name in &names {
            let collection = self.collection(name)?;
            plans.insert(name.clone(), SortPlan::compile(&collection, &request.sort)?);
            if indexed_order {
                self.ensure_picker(&collection)?;
                let picker = &self.file_pickers[name];
                let scoped = filter.for_fields(&collection.fields);
                let risky = scoped
                    .needs_cardinality_check_in(|path| picker.typed_scalar_safe(path))
                    || scoped
                        .top_level_scalar_fields()
                        .iter()
                        .any(|field| picker.has_array_value(field));
                indexed_order = picker.has_sort_index(&request.sort) && picker.verified() && !risky;
            }
        }
        // An unchecked restored picker can still be missing recent files. In
        // that state, project current source values into the fallback buffer.
        if indexed_order && request.offset < self.options.total_limit && request.limit > 0 {
            return self.query_json_indexed_order(request, on_page, started, &before);
        }
        let mut buffer = SortBuffer::new(
            plans,
            request.offset,
            request.limit,
            self.options.total_limit,
        );
        let mut scan = request.clone();
        scan.offset = 0;
        scan.limit = if indexed_order {
            self.options
                .total_limit
                .saturating_add(1)
                .min(i64::MAX as usize)
        } else {
            i64::MAX as usize
        };
        scan.count = !indexed_order;
        let mut selected_validation = Metrics::default();
        let root = self.model.root.clone();
        let (mut result, _) = if matches!(
            scan.backend,
            JsonQueryBackend::Auto | JsonQueryBackend::Bigram
        ) {
            let mut on_candidate = |collection: &Collection,
                                    id: &str,
                                    stamp: &Stamp,
                                    mut document: Document,
                                    hash: String| {
                let keys = buffer.project(&collection.name, id, &document.data)?;
                let changed = buffer.insert_keys(&collection.name, id, keys, || {
                    if collection.schema.is_some() {
                        let started = Instant::now();
                        document.validate(collection);
                        selected_validation.validations += 1;
                        selected_validation.validation_nanos += nanos(started);
                    } else {
                        document.state.status = RecordStatus::NoSchema;
                    }
                    Record {
                        collection: collection.name.clone(),
                        id: id.into(),
                        path: collection
                            .path
                            .join(format!("{id}.json"))
                            .strip_prefix(&root)
                            .expect("record is within repository")
                            .to_string_lossy()
                            .into_owned(),
                        size_bytes: stamp.size,
                        modified_unix_nanos: stamp.modified.clone(),
                        hash,
                        data: document.data,
                        state: document.state,
                        raw: None,
                    }
                });
                if changed && (buffer.seen() <= 16 || buffer.seen().is_multiple_of(64)) {
                    let page = buffer.page().cloned().collect::<Vec<_>>();
                    on_page(&page)?;
                }
                Ok(())
            };
            self.query_json_inner(
                &scan,
                true,
                &mut |_, _| Ok(()),
                Some(&mut on_candidate),
                None,
            )?
        } else {
            self.query_json_inner(
                &scan,
                true,
                &mut |_, record| {
                    let changed =
                        buffer.insert(&record.collection, &record.id, &record.data, || {
                            record.clone()
                        })?;
                    if changed && (buffer.seen() <= 16 || buffer.seen().is_multiple_of(64)) {
                        let page = buffer.page().cloned().collect::<Vec<_>>();
                        on_page(&page)?;
                    }
                    Ok(())
                },
                None,
                None,
            )?
        };
        self.metrics.add_from(&selected_validation);
        let page = buffer.page().cloned().collect::<Vec<_>>();
        on_page(&page)?;
        result.total = buffer.total(request.count);
        result.truncated = buffer.truncated();
        result.records = buffer.into_page();
        result.metrics = self.metrics.since(&before);
        result.elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
        result
            .query_plan
            .push("locale-aware sorted page; bounded result prefix".into());
        Ok(result)
    }

    fn query_json_indexed_order(
        &mut self,
        request: &JsonQuery,
        on_page: &mut dyn FnMut(&[Record]) -> Result<()>,
        started: Instant,
        before: &Metrics,
    ) -> Result<JsonQueryResult> {
        let mut scan = request.clone();
        scan.count = true;
        scan.limit = request.limit.min(self.options.total_limit - request.offset);
        // One match beyond the cap is enough to report truncation; later
        // matches cannot change the accessible page or capped total.
        let stop_after = self.options.total_limit.saturating_add(1);
        let mut page = Vec::new();
        let (mut result, _) = self.query_json_inner(
            &scan,
            true,
            &mut |_, record| {
                page.push(record.clone());
                on_page(&page)
            },
            None,
            Some(stop_after),
        )?;
        on_page(&page)?;
        result.records = page;
        if !request.count {
            result.total = None;
        }
        result.metrics = self.metrics.since(before);
        result.elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
        result
            .query_plan
            .push("indexed sort order; direct verified page".into());
        Ok(result)
    }
}
