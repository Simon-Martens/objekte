//! Execute sorted text pages with an index or a bounded fallback buffer.

use super::*;
use crate::sorting::{SortBuffer, SortRecord};

type PageCallback<'a> = dyn FnMut(&[TextHit]) -> Result<()> + 'a;


impl Repository {
    pub(super) fn search_text_sorted<const CUSTOM: bool>(
        &mut self,
        request: &TextQuery,
        mut on_page: Option<&mut PageCallback<'_>>,
        extension: Option<&BoundTextQuery>,
    ) -> Result<TextQueryResult> {
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
        let mut indexed_order = self.options.cache == CacheMode::Memory
            && self.options.freshness != Freshness::HashOnRequest;
        for name in &names {
            let collection = self.collection(name)?;
            plans.insert(name.clone(), self.sort_plan(&collection, &request.sort)?);
            if indexed_order {
                self.ensure_picker(&collection)?;
                indexed_order = self.file_pickers[name].has_sort_index(&request.sort)
                    && self.file_pickers[name].verified();
            }
        }
        // Only a verified picker may supply the final order directly. A
        // restored picker still being checked uses source-projected keys.
        if indexed_order && request.offset < self.options.total_limit && request.limit > 0 {
            return self.search_text_indexed_order::<CUSTOM>(request, on_page, started, &before, extension);
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
        let root = self.model.root.clone();
        let collection_paths = self.collection_paths(&names);
        let (mut result, _) = self.search_text_inner::<CUSTOM>(
            &scan,
            true,
            &mut |_, hit| {
                // Returned text hits are already validated.
                let source = SortRecord {
                    id: &hit.record.id,
                    collection_path: &collection_paths[&hit.record.collection],
                    root: &root,
                    size: hit.record.size_bytes,
                    modified: &hit.record.modified_unix_nanos,
                    status: hit.record.state.status,
                };
                let keys = buffer.project(&hit.record.collection, &source, &hit.record.data)?;
                let changed = buffer
                    .insert_owned_keys(keys, hit, |hit| (&hit.record.collection, &hit.record.id));
                if changed
                    && (buffer.seen() <= 16 || buffer.seen().is_multiple_of(64))
                    && let Some(callback) = on_page.as_deref_mut()
                {
                    buffer.with_page(callback)?;
                }
                Ok(())
            },
            None,
            extension,
        )?;
        result.total = buffer.total(request.count);
        result.truncated = buffer.truncated();
        let page = buffer.into_page();
        if let Some(callback) = on_page {
            callback(&page)?;
        }
        result.hits = page;
        result.metrics = self.metrics.since(&before);
        result.elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
        result
            .query_plan
            .push("locale-aware sorted page; bounded result prefix".into());
        Ok(result)
    }

    fn search_text_indexed_order<const CUSTOM: bool>(
        &mut self,
        request: &TextQuery,
        on_page: Option<&mut PageCallback<'_>>,
        started: Instant,
        before: &Metrics,
        extension: Option<&BoundTextQuery>,
    ) -> Result<TextQueryResult> {
        let mut scan = request.clone();
        scan.count = true;
        scan.limit = request.limit.min(self.options.total_limit - request.offset);
        // Verify one match past the cap to distinguish a full page from a
        // truncated result without scanning the remaining candidates.
        let stop_after = self.options.total_limit.saturating_add(1);
        let (mut result, _) = if let Some(on_page) = on_page {
            let mut page = Vec::new();
            let (mut result, returned) = self.search_text_inner::<CUSTOM>(
                &scan,
                true,
                &mut |_, hit| {
                    page.push(hit);
                    on_page(&page)
                },
                Some(stop_after),
                extension,
            )?;
            on_page(&page)?;
            result.hits = page;
            (result, returned)
        } else {
            self.search_text_inner::<CUSTOM>(
                &scan,
                false,
                &mut |_, _| Ok(()),
                Some(stop_after),
                extension,
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
