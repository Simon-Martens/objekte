//! Count covered scalar predicates from the in-memory index and materialize the page.

use super::*;
use crate::file_picker::ExactSelection;


/// Immutable query inputs shared by the exact index and page materializer.
pub(super) struct ExactQuery<'a> {
    pub filter: &'a JsonFilter,
    pub collection: &'a Collection,
    pub root: &'a Path,
}


impl Repository {
    pub(super) fn query_json_exact_records(
        metrics: &mut Metrics,
        query: ExactQuery<'_>,
        selection: &ExactSelection<'_>,
        result: &mut QueryOutput<'_>,
        matched: &mut usize,
    ) -> Result<()> {
        *matched += selection.count;
        if selection.page.is_empty() {
            return Ok(());
        }
        let source = Source::open(&query.collection.path)?;
        for &(id, stamp) in &selection.page {
            let started = Instant::now();
            let raw = Self::read_raw(metrics, &source, id, Some(stamp))?;
            let doc = Self::parse_value(metrics, query.collection, id, &raw.json, true);
            if doc.state.status == RecordStatus::IncorrectJson
                || !query.filter.matches(&doc.data)?
            {
                return Err(Error::Unstable(source.path.join(format!("{id}.json"))));
            }
            result.push(Record {
                collection: query.collection.name.clone(),
                id: id.to_owned(),
                path: query
                    .collection
                    .path
                    .join(format!("{id}.json"))
                    .strip_prefix(query.root)
                    .expect("collection is inside repository")
                    .to_string_lossy()
                    .into_owned(),
                hash: raw.hash,
                data: doc.data,
                state: doc.state,
                raw: None,
            })?;
            metrics.bigram_materialize_nanos += nanos(started);
        }
        source.finish()?;
        Ok(())
    }
}
