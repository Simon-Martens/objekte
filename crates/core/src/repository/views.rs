//! Scalar view planning, indexed page reads, and direct execution.

use super::*;


impl Repository {
    pub fn open_view(&mut self, collection: &str, request: &ViewRequest) -> Result<ViewResult> {
        let before = self.metrics.clone();
        self.reload()?;
        let c = self.collection(collection)?;
        let plan = Plan::compile(&c, request)?;
        let table = store::record_table();
        let mut result = if self.options.cache == CacheMode::Disabled {
            self.direct(&c, Some(&plan), false)?.unwrap()
        } else {
            let writer = self.store()?;
            let s = writer.enter(self.options.sqlite_cache_mib, self.options.sql_diagnostics)?;
            self.sync(&c, &s, &store::requested_view_indexes(&c, &plan), false)?;
            let t = Instant::now();
            let (where_sql, mut values) = plan.sql_where(&c);
            let total = if plan.count {
                let index = store::filter_view_index(&c, &plan.filters);
                Some(s.db.query_row(
                    &format!("SELECT count(*) FROM {table} v INDEXED BY {index}{where_sql}"),
                    params_from_iter(values.iter()),
                    |r| r.get::<_, i64>(0),
                )? as usize)
            } else {
                None
            };
            // PERFORMANCE: Choose the page from compact typed keys before fetching JSON payloads.
            // This fixed lowering does not need an adaptive optimizer or result cache.
            let order = plan.sql_order(&c);
            let (keys, page_order) = plan.sql_page_keys(&c);
            // INFO: Use the requested order index, or the covering keys for compound
            // views. Payloads are read only for the selected page.
            let index = store::page_view_index(&c, &plan);
            let access = format!(" INDEXED BY {index}");
            let sql = format!(
                "WITH page AS MATERIALIZED (SELECT v.id{keys} FROM {table} v{access}{where_sql} ORDER BY {order} LIMIT ? OFFSET ?) SELECT r.id,r.path,r.json,r.validation_state,r.issues FROM page p CROSS JOIN {table} r ON r.id=p.id AND r.collection=? ORDER BY {page_order}"
            );
            values.push((plan.limit as i64).into());
            values.push((plan.offset as i64).into());
            values.push(collection.to_owned().into());
            let mut explain = s.db.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?;
            let query_plan = explain
                .query_map(params_from_iter(values.iter()), |r| r.get::<_, String>(3))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let mut stmt = s.db.prepare(&sql)?;
            let rows = stmt
                .query_map(params_from_iter(values.iter()), |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get_ref(2)?.as_bytes()?.to_vec(),
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let mut out = Vec::with_capacity(rows.len());
            let fields = plan
                .columns
                .iter()
                .map(String::as_str)
                .chain(std::iter::once(c.display.as_str()))
                .filter(|field| *field != "@id")
                .collect();
            let mut states = BTreeMap::new();
            for (id, path, bytes, status, issues) in rows {
                let state = RecordState::read(&status, &issues)?;
                let value = if state.status == RecordStatus::IncorrectJson {
                    Value::Null
                } else {
                    let json =
                        std::str::from_utf8(&bytes).map_err(|e| Error::Cache(e.to_string()))?;
                    if json.trim_start().starts_with('{') {
                        crate::json::project(json, &fields)
                    } else {
                        crate::json::parse(json)
                    }
                    .map_err(|e| Error::Cache(format!("invalid cached JSON: {e}")))?
                };
                states.insert(id.clone(), state);
                out.push(plan.row(&c, &id, path, &value));
            }
            self.metrics.query_nanos += nanos(t);
            ViewResult {
                columns: plan.columns.clone(),
                rows: out,
                states,
                total,
                metrics: Metrics::default(),
                query_plan,
                view_hashes: plan.hashes.clone(),
            }
        };
        result.metrics = self.metrics.since(&before);
        Ok(result)
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
