//! Value counts read from ordered postings, for suggestions and facets.

use super::*;


/// One value of an indexed field and how many records hold it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ValueCount {
    pub value: Value,
    /// Records holding the value; a record that holds it twice counts once.
    pub records: usize,
}


impl Repository {
    /// The most common values of an indexed field in one collection, most
    /// records first and ties in value order, at most `limit` of them.
    ///
    /// The counts come from the field's ordered posting, so no source file
    /// is read. They reflect the index as of its last refresh or reported
    /// change. `null` and missing values are left out. Arrays count each
    /// distinct element, as filters see them.
    ///
    /// `path` uses filter syntax, such as `year` or `contributors[*].type`;
    /// the implicit spelling `contributors.type` works where it reaches the
    /// same values. Returns `None` when this handle has no current posting
    /// for the path: the collection's index is not built yet, the path is
    /// neither an indexed field nor an index path, or its posting is stale.
    pub fn value_counts(
        &self,
        collection: &str,
        path: &str,
        limit: usize,
    ) -> Result<Option<Vec<ValueCount>>> {
        self.model.collection(collection)?;
        let path = crate::JsonFilter::parse_path(path)?;
        let Some(picker) = self.file_pickers.get(collection) else {
            return Ok(None);
        };
        let Some(counts) = picker.value_counts(&path) else {
            return Ok(None);
        };
        let mut counts = counts
            .into_iter()
            .map(|(value, records)| ValueCount { value, records })
            .collect::<Vec<_>>();
        // Stable: equal counts stay in the posting's value order.
        counts.sort_by_key(|count| std::cmp::Reverse(count.records));
        counts.truncate(limit);
        Ok(Some(counts))
    }
}
