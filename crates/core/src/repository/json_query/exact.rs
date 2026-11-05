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
    pub(super) fn query_json_metadata_records(
        metrics: &mut Metrics,
        query: ExactQuery<'_>,
        page: &[(&str, &Stamp)],
        result: &mut QueryOutput<'_>,
    ) -> Result<()> {
        if page.is_empty() {
            return Ok(());
        }
        let source = Source::open_scoped(&query.collection.path, query.collection.recursive)?;
        for &(id, stamp) in page {
            let started = Instant::now();
            let raw = Self::read_raw(metrics, &source, id, Some(stamp))?;
            let doc = Self::parse_value(metrics, query.collection, id, &raw.json, true);
            let path = query
                .collection
                .path
                .join(format!("{id}.json"))
                .strip_prefix(query.root)
                .expect("collection is inside repository")
                .to_string_lossy()
                .into_owned();
            if !query.filter.matches_source(
                &doc.data,
                id,
                &path,
                &raw.stamp,
                raw.created,
                doc.state.status,
            )? {
                return Err(Error::Unstable(source.path.join(format!("{id}.json"))));
            }
            let malformed = doc.state.status.is_malformed();
            result.push(Record {
                collection: query.collection.name.clone(),
                id: id.to_owned(),
                path,
                hash: raw.hash,
                size_bytes: raw.stamp.size,
                modified_unix_nanos: raw.stamp.modified.clone(),
                data: doc.data,
                state: doc.state,
                raw: malformed.then_some(raw.json),
            })?;
            metrics.bigram_materialize_nanos += nanos(started);
        }
        source.finish()?;
        Ok(())
    }

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
        let source = Source::open_scoped(&query.collection.path, query.collection.recursive)?;
        for &(id, stamp) in &selection.page {
            let started = Instant::now();
            let raw = Self::read_raw(metrics, &source, id, Some(stamp))?;
            let doc = Self::parse_value(metrics, query.collection, id, &raw.json, true);
            if doc.state.status.is_malformed() || !query.filter.matches(&doc.data)? {
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
                size_bytes: raw.stamp.size,
                modified_unix_nanos: raw.stamp.modified.clone(),
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
