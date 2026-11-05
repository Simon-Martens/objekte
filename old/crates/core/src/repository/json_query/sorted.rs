//! Route sorted JSON pages to the verified index or bounded fallback.

use super::*;
use crate::sorting::{SortBuffer, SortRecord};

type PageCallback<'a> = dyn FnMut(&[Record]) -> Result<()> + 'a;


impl Repository {
    pub(super) fn query_json_sorted(
        &mut self,
        request: &JsonQuery,
        mut on_page: Option<&mut PageCallback<'_>>,
    ) -> Result<JsonQueryResult> {
        let started = Instant::now();
        let before = self.metrics.clone();
        if self.options.total_limit == 0 {
            return Err(Error::Request(
                "repository total_limit must be positive".into(),
            ));
        }
        let names = request
            .collections
            .clone()
            .unwrap_or_else(|| self.model.collections.keys().cloned().collect::<Vec<_>>());
        let mut plans = BTreeMap::new();
        let filter =
            JsonFilter::compile_with_fts(&request.filter, &request.parameters, request.fts)?;
        let mut indexed_order = matches!(
            request.backend,
            JsonQueryBackend::Auto | JsonQueryBackend::Bigram
        ) && self.options.cache == CacheMode::Memory
            && self.options.freshness != Freshness::HashOnRequest;
        for name in &names {
            let collection = self.collection(name)?;
            plans.insert(name.clone(), self.sort_plan(&collection, &request.sort)?);
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
        let collection_paths = self.collection_paths(&names);
        let status_sort = buffer.uses_status();
        let (mut result, _) = if matches!(
            scan.backend,
            JsonQueryBackend::Auto | JsonQueryBackend::Bigram
        ) {
            let mut on_candidate = |collection: &Collection,
                                    id: &str,
                                    stamp: &Stamp,
                                    mut document: Document,
                                    hash: String,
                                    raw: Option<Vec<u8>>| {
                let mut validate = |document: &mut Document| {
                    // Malformed JSON keeps its status and has no schema state.
                    if document.state.status.is_malformed() {
                        return;
                    }
                    if collection.schema.is_none() {
                        document.state.status = RecordStatus::NoSchema;
                    } else if document.state.status == RecordStatus::Pending {
                        let started = Instant::now();
                        document.validate(collection);
                        selected_validation.validations += 1;
                        selected_validation.validation_nanos += nanos(started);
                    }
                };
                // Only a status sort needs every candidate's validation state.
                if status_sort {
                    validate(&mut document);
                }
                let record = SortRecord {
                    id,
                    collection_path: &collection.path,
                    root: &root,
                    size: stamp.size,
                    modified: &stamp.modified,
                    status: document.state.status,
                    locator: None,
                    created: None,
                };
                let keys = buffer.project(&collection.name, &record, &document.data)?;
                let changed = buffer.insert_keys(&collection.name, id, keys, || {
                    validate(&mut document);
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
                        raw,
                    }
                });
                if changed
                    && (buffer.seen() <= 16 || buffer.seen().is_multiple_of(64))
                    && let Some(callback) = on_page.as_deref_mut()
                {
                    buffer.with_page(callback)?;
                }
                Ok(())
            };
            self.query_json_inner(
                &scan,
                true,
                &mut |_, _| Ok(()),
                None,
                Some(&mut on_candidate),
                None,
            )?
        } else {
            self.query_json_inner(
                &scan,
                true,
                &mut |_, _| Ok(()),
                Some(&mut |record: Record| {
                    let source = SortRecord {
                        id: &record.id,
                        collection_path: &collection_paths[&record.collection],
                        root: &root,
                        size: record.size_bytes,
                        modified: &record.modified_unix_nanos,
                        status: record.state.status,
                        locator: None,
                        created: None,
                    };
                    let keys = buffer.project(&record.collection, &source, &record.data)?;
                    let changed = buffer
                        .insert_owned_keys(keys, record, |record| (&record.collection, &record.id));
                    if changed
                        && (buffer.seen() <= 16 || buffer.seen().is_multiple_of(64))
                        && let Some(callback) = on_page.as_deref_mut()
                    {
                        buffer.with_page(callback)?;
                    }
                    Ok(())
                }),
                None,
                None,
            )?
        };
        self.metrics.add_from(&selected_validation);
        result.total = buffer.total(request.count);
        result.truncated = buffer.truncated();
        let page = buffer.into_page();
        if let Some(callback) = on_page {
            callback(&page)?;
        }
        result.records = page;
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
        on_page: Option<&mut PageCallback<'_>>,
        started: Instant,
        before: &Metrics,
    ) -> Result<JsonQueryResult> {
        let mut scan = request.clone();
        scan.count = true;
        scan.limit = request.limit.min(self.options.total_limit - request.offset);
        // One match beyond the cap is enough to report truncation; later
        // matches cannot change the accessible page or capped total.
        let stop_after = self.options.total_limit.saturating_add(1);
        let (mut result, _) = if let Some(on_page) = on_page {
            let mut page = Vec::new();
            let (mut result, returned) = self.query_json_inner(
                &scan,
                true,
                &mut |_, _| Ok(()),
                Some(&mut |record| {
                    page.push(record);
                    on_page(&page)
                }),
                None,
                Some(stop_after),
            )?;
            on_page(&page)?;
            result.records = page;
            (result, returned)
        } else {
            self.query_json_inner(
                &scan,
                false,
                &mut |_, _| Ok(()),
                None,
                None,
                Some(stop_after),
            )?
        };
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
