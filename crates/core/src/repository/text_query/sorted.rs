//! Execute sorted text pages with an index or a bounded fallback buffer.

use super::*;
use crate::sorting::{SortBuffer, SortPlan};


impl Repository {
    pub(super) fn search_text_sorted(
        &mut self,
        request: &TextQuery,
        on_page: &mut dyn FnMut(&[TextHit]) -> Result<()>,
    ) -> Result<TextQueryResult> {
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
        let mut indexed_order = self.options.cache == CacheMode::Memory
            && self.options.freshness != Freshness::HashOnRequest;
        for name in &names {
            let collection = self.collection(name)?;
            plans.insert(name.clone(), SortPlan::compile(&collection, &request.sort)?);
            if indexed_order {
                self.ensure_picker(&collection)?;
                indexed_order = self.file_pickers[name].has_sort_index(&request.sort)
                    && self.file_pickers[name].verified();
            }
        }
        // Only a verified picker may supply the final order directly. A
        // restored picker still being checked uses source-projected keys.
        if indexed_order && request.offset < self.options.total_limit && request.limit > 0 {
            return self.search_text_indexed_order(request, on_page, started, &before);
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
        let (mut result, _) = self.search_text_inner(
            &scan,
            true,
            &mut |_, hit| {
                let record = &hit.record;
                let changed =
                    buffer.insert(&record.collection, &record.id, &record.data, || hit.clone())?;
                if changed && (buffer.seen() <= 16 || buffer.seen().is_multiple_of(64)) {
                    let page = buffer.page().cloned().collect::<Vec<_>>();
                    on_page(&page)?;
                }
                Ok(())
            },
            None,
        )?;
        let page = buffer.page().cloned().collect::<Vec<_>>();
        on_page(&page)?;
        result.total = buffer.total(request.count);
        result.truncated = buffer.truncated();
        result.hits = buffer.into_page();
        result.metrics = self.metrics.since(&before);
        result.elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
        result
            .query_plan
            .push("locale-aware sorted page; bounded result prefix".into());
        Ok(result)
    }

    fn search_text_indexed_order(
        &mut self,
        request: &TextQuery,
        on_page: &mut dyn FnMut(&[TextHit]) -> Result<()>,
        started: Instant,
        before: &Metrics,
    ) -> Result<TextQueryResult> {
        let mut scan = request.clone();
        scan.count = true;
        scan.limit = request.limit.min(self.options.total_limit - request.offset);
        // Verify one match past the cap to distinguish a full page from a
        // truncated result without scanning the remaining candidates.
        let stop_after = self.options.total_limit.saturating_add(1);
        let mut page = Vec::new();
        let (mut result, _) = self.search_text_inner(
            &scan,
            true,
            &mut |_, hit| {
                page.push(hit.clone());
                on_page(&page)
            },
            Some(stop_after),
        )?;
        on_page(&page)?;
        result.hits = page;
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
