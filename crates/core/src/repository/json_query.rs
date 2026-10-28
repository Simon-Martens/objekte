use super::*;
use crate::{JsonFilter, JsonQuery, JsonQueryDependencies, JsonQueryResult, RecordState};

impl Repository {
    /// Query actual JSON values in all registered collections, or an explicit scope.
    /// The schema/UI never restrict queryable paths. Ordering is (collection, id).
    pub fn query_json(&mut self, request: &JsonQuery) -> Result<JsonQueryResult> {
        let before = self.metrics.clone();
        let t = Instant::now();
        if request.limit == 0
            || request.limit > i64::MAX as usize
            || request.offset > i64::MAX as usize
        {
            return Err(Error::Request(
                "query limit must be positive and pagination must fit i64".into(),
            ));
        }
        let filter = JsonFilter::compile(&request.filter, &request.parameters)?;
        self.metrics.filter_compile_nanos += nanos(t);
        self.reload()?;
        let names = request
            .collections
            .clone()
            .unwrap_or_else(|| self.model.collections.keys().cloned().collect())
            .into_iter()
            .collect::<BTreeSet<_>>();
        let collections = names
            .iter()
            .map(|name| self.collection(name))
            .collect::<Result<Vec<_>>>()?;
        let mut result = JsonQueryResult {
            records: Vec::new(),
            total: request.count.then_some(0),
            dependencies: JsonQueryDependencies {
                collections: names.into_iter().collect(),
                paths: filter.paths().to_vec(),
                revisions: BTreeMap::new(),
            },
            filter_hash: filter.hash().into(),
            metrics: Metrics::default(),
            sql: None,
            bindings: Vec::new(),
            query_plan: Vec::new(),
        };
        if !collections.is_empty() {
            if self.options.cache == CacheMode::Sqlite {
                self.query_json_sql(request, &filter, &collections, &mut result)?;
                for c in &collections {
                    self.schedule_validation(c)?;
                }
            } else {
                self.query_json_direct(request, &filter, &collections, &mut result)?;
            }
        }
        result.metrics = self.metrics.since(&before);
        Ok(result)
    }

    fn query_json_sql(
        &mut self,
        request: &JsonQuery,
        filter: &JsonFilter,
        collections: &[Collection],
        result: &mut JsonQueryResult,
    ) -> Result<()> {
        let writer = self.store()?;
        let s = writer.enter(self.options.sqlite_cache_mib)?;
        for c in collections {
            self.sync(c, &s, filter.uses_indexes(c), false)?;
            result
                .dependencies
                .revisions
                .insert(c.name.clone(), s.state(&c.name)?.unwrap().revision);
        }
        let t = Instant::now();
        let mut bindings = Vec::new();
        for c in collections {
            filter.check_sql_cardinality(&s.db, c)?;
        }
        let selections = collections
            .iter()
            .map(|c| {
                let predicate = filter.sql(c, &mut bindings);
                let scope = crate::json_filter::indexes::quote(&c.name);
                format!("SELECT r.collection,r.id FROM records r JOIN record_validation h USING(collection,id) WHERE r.collection={scope} AND h.parse_status='parsed' AND ({predicate})")
            })
            .collect::<Vec<_>>()
            .join(" UNION ALL ");
        bindings.push(request.limit.to_string());
        let limit = bindings.len();
        bindings.push(request.offset.to_string());
        let offset = bindings.len();
        let count = if request.count {
            "(SELECT count(*) FROM hits)"
        } else {
            "NULL"
        };
        // Materialize only matched identities. This deliberately examines all scoped
        // records before pagination, including scalar-cardinality errors outside the
        // returned page or in an otherwise false OR/AND branch. Never materialize JSON.
        let sql = format!("WITH hits AS MATERIALIZED ({selections}), page AS (
            SELECT collection,id FROM hits ORDER BY collection COLLATE BINARY,id COLLATE BINARY LIMIT ?{limit} OFFSET ?{offset}
        )
        SELECT 1 AS summary,NULL AS collection,NULL AS id,NULL AS path,NULL AS digest,NULL AS json,NULL AS validation_state,NULL AS issues,{count} AS total
        UNION ALL
        SELECT 0,r.collection,r.id,r.path,r.digest,r.json,h.validation_state,h.issues,NULL
        FROM page p JOIN records r USING(collection,id) JOIN record_validation h USING(collection,id)
        ORDER BY summary DESC,collection COLLATE BINARY,id COLLATE BINARY");
        let mut explain = s.db.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?;
        result.query_plan = explain
            .query_map(params_from_iter(&bindings), |r| r.get(3))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(explain);
        let mut statement = s.db.prepare(&sql)?;
        let mut rows = statement.query(params_from_iter(&bindings))?;
        while let Some(row) = rows.next()? {
            if row.get::<_, bool>(0)? {
                result.total = row.get::<_, Option<i64>>(8)?.map(|n| n as usize);
                continue;
            }
            let bytes = row
                .get_ref(5)?
                .as_bytes()
                .map_err(|e| Error::Cache(e.to_string()))?;
            let data = crate::json::parse(
                std::str::from_utf8(bytes).map_err(|e| Error::Cache(e.to_string()))?,
            )
            .map_err(|e| Error::Cache(format!("invalid cached JSON: {e}")))?;
            result.records.push(Record {
                collection: row.get(1)?,
                id: row.get(2)?,
                path: row.get(3)?,
                hash: row.get(4)?,
                data,
                state: RecordState::read(&row.get::<_, String>(6)?, &row.get::<_, String>(7)?)?,
                raw: None,
            });
        }
        result.sql = Some(sql);
        result.bindings = bindings;
        self.metrics.query_nanos += nanos(t);
        Ok(())
    }

    fn query_json_direct(
        &mut self,
        request: &JsonQuery,
        filter: &JsonFilter,
        collections: &[Collection],
        result: &mut JsonQueryResult,
    ) -> Result<()> {
        let mut matched = 0;
        for c in collections {
            let (source, files) = self.inventory(c)?;
            for (id, stamp) in &files {
                let raw = self.raw_from(&source, id, Some(stamp))?;
                let doc = self.parse(c, id, &raw.json, true);
                let t = Instant::now();
                if doc.state.status != RecordStatus::IncorrectJson && filter.matches(&doc.data)? {
                    if matched >= request.offset && result.records.len() < request.limit {
                        result.records.push(Record {
                            collection: c.name.clone(),
                            id: id.clone(),
                            path: self.path(c, id),
                            hash: raw.hash,
                            data: doc.data,
                            state: doc.state,
                            raw: None,
                        });
                    }
                    matched += 1;
                }
                self.metrics.query_nanos += nanos(t);
            }
            source.finish()?;
            if self.inventory(c)?.1 != files {
                return Err(Error::Unstable(c.path.clone()));
            }
        }
        result.total = request.count.then_some(matched);
        result.query_plan = vec!["direct source scan; compiled JSON filter".into()];
        Ok(())
    }
}
