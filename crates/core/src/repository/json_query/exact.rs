//! Count covered scalar predicates from the in-memory index and materialize the page.

use super::*;


/// Immutable query inputs shared by the exact index and page materializer.
pub(super) struct ExactQuery<'a> {
    pub request: &'a JsonQuery,
    pub filter: &'a JsonFilter,
    pub collection: &'a Collection,
    pub root: &'a Path,
}


impl Repository {
    pub(super) fn query_json_exact_records(
        metrics: &mut Metrics,
        query: ExactQuery<'_>,
        entries: &[(&str, &Stamp)],
        result: &mut QueryOutput<'_>,
        matched: &mut usize,
    ) -> Result<()> {
        let source = Source::open(&query.collection.path)?;
        for &(id, stamp) in entries {
            if !query.request.count && result.returned() == query.request.limit {
                break;
            }
            if *matched >= query.request.offset && result.returned() < query.request.limit {
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
            } else {
                // Indexed counts still check every matching source's identity.
                // Only returned records pay for a read, hash, and JSON parse.
                let started = Instant::now();
                source.check_stamp(id, stamp)?;
                metrics.source_check_nanos += nanos(started);
            }
            *matched += 1;
        }
        source.finish()?;
        Ok(())
    }
}
