//! SQLite expression indexes over raw JSON. No materialized field columns.
use super::*;
use crate::{
    Collection,
    schema::{Field, ValueType},
};
use rusqlite::{Connection, functions::FunctionFlags};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub(crate) fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}
pub(crate) fn field(name: &str) -> FieldOperand {
    FieldOperand {
        path: vec![JsonPathStep::Key(name.into())],
        modifier: None,
    }
}
pub(crate) fn key_expr(alias: &str, field: &FieldOperand) -> String {
    let json = if alias.is_empty() {
        "json".into()
    } else {
        format!("{alias}.json")
    };
    let id = if alias.is_empty() {
        "id".into()
    } else {
        format!("{alias}.id")
    };
    format!(
        "objekte_json_key({json},{id},{}) COLLATE objekte_json_v1",
        quote(&serde_json::to_string(field).unwrap())
    )
}
pub(crate) fn indexed(c: &Collection, f: &FieldOperand) -> bool {
    f.modifier.is_none()
        && matches!(&f.path[..], [JsonPathStep::Key(name)] if c.indexes.iter().any(|i| i.name == *name && name != "@id"))
}
pub(crate) fn view_expr(c: &Collection, name: &str, alias: &str, state: bool) -> String {
    let spec = serde_json::to_string(&(name, c.fields[name].kind)).unwrap();
    let json = if alias.is_empty() {
        "json".into()
    } else {
        format!("{alias}.json")
    };
    let id = if alias.is_empty() {
        "id".into()
    } else {
        format!("{alias}.id")
    };
    format!(
        "objekte_view_{}({json},{id},{})",
        if state { "state" } else { "value" },
        quote(&spec)
    )
}
pub(crate) fn key(v: &Value) -> String {
    match v {
        Value::Bool(v) => format!("B{}", u8::from(*v)),
        Value::Number(v) => format!("N{v}"),
        Value::String(v) => format!("S{v}"),
        Value::Null => "Z".into(),
        _ => "O".into(),
    }
}
fn key_cmp(a: &str, b: &str) -> Ordering {
    if let (Some(a), Some(b)) = (a.strip_prefix('N'), b.strip_prefix('N')) {
        // Empty payload is the numeric range boundary, not a JSON number.
        if a.is_empty() || b.is_empty() {
            return a.cmp(b);
        }
        decimal_cmp(a, b)
    } else {
        a.cmp(b)
    }
}
// Share one decoded document across all expression functions. Include bytes in
// the identity: external corruption must not be hidden by an unchanged digest.
type DocumentCache = Arc<Mutex<Option<(Vec<u8>, Option<String>, Option<Arc<Value>>)>>>;
type PreparedKeys = Arc<Vec<(crate::query::Scalar, String)>>;
type Building = Arc<Mutex<Option<BuildCache>>>;
struct BuildCache {
    fields: Vec<(FieldOperand, Field)>,
    entries: HashMap<[u8; 32], PreparedKeys>,
    rows: Option<HashMap<String, PreparedKeys>>,
    last: Option<(Vec<u8>, PreparedKeys)>,
    used: usize,
    budget: usize,
    maintaining: bool,
}
impl BuildCache {
    fn extract(&self, doc: Option<&Value>) -> PreparedKeys {
        Arc::new(
            self.fields
                .iter()
                .map(|(f, definition)| {
                    let JsonPathStep::Key(name) = &f.path[0] else {
                        unreachable!()
                    };
                    (
                        crate::query::scalar(
                            "",
                            doc.unwrap_or(&Value::Null),
                            name,
                            Some(definition),
                        ),
                        document_key(doc, f),
                    )
                })
                .collect(),
        )
    }
    fn remember(&mut self, digest: [u8; 32], keys: &PreparedKeys) {
        if self.entries.contains_key(&digest) {
            return;
        }
        // Conservative retained-key accounting; not a process-wide RSS limit.
        let charge = 128
            + keys
                .iter()
                .map(|(value, key)| {
                    96 + key.capacity()
                        + if let crate::query::Scalar::String(s) = value {
                            s.capacity()
                        } else {
                            0
                        }
                })
                .sum::<usize>();
        if charge <= self.budget.saturating_sub(self.used) {
            self.used += charge;
            self.entries.insert(digest, keys.clone());
        }
    }
}
/// Transient keys during construction, or one decoded document during updates.
/// No extracted fields are persisted outside SQLite's expression indexes.
pub(crate) struct IndexCache {
    document: DocumentCache,
    building: Building,
}
pub(crate) struct BuildGuard(Building);
impl Drop for BuildGuard {
    fn drop(&mut self) {
        if let Ok(mut building) = self.0.lock() {
            *building = None;
        }
    }
}
impl IndexCache {
    /// Returns true when the write maintains existing indexes, false when keys
    /// are being retained for the later bulk index build instead.
    pub(crate) fn prime(&self, bytes: &[u8], value: Option<&Value>) -> Result<bool> {
        if let Some(build) = self
            .building
            .lock()
            .map_err(|_| Error::Cache("index build cache poisoned".into()))?
            .as_mut()
        {
            // No row-key reuse across writes, even inside the same guard.
            build.rows = None;
            let keys = build.extract(value);
            if build.maintaining {
                build.last = Some((bytes.to_vec(), keys));
                return Ok(true);
            }
            build.remember(Sha256::digest(bytes).into(), &keys);
            return Ok(false);
        }
        *self
            .document
            .lock()
            .map_err(|_| Error::Cache("index document cache poisoned".into()))? =
            Some((bytes.to_vec(), None, value.map(|v| Arc::new(v.clone()))));
        Ok(true)
    }
    pub(crate) fn build(
        &self,
        c: &Collection,
        names: &[String],
        maintaining: bool,
    ) -> Result<BuildGuard> {
        let fields = crate::store::requested_view_fields(c, names);
        *self
            .building
            .lock()
            .map_err(|_| Error::Cache("index build cache poisoned".into()))? = Some(BuildCache {
            fields: c
                .indexes
                .iter()
                .filter(|f| fields.contains(&f.name))
                .map(|f| (field(&f.name), c.fields[&f.name].clone()))
                .collect(),
            entries: HashMap::new(),
            rows: None,
            last: None,
            used: 0,
            budget: if maintaining { 0 } else { 128 * 1024 * 1024 },
            maintaining,
        });
        Ok(BuildGuard(self.building.clone()))
    }
    /// Only enable after all writes, while the writer transaction holds the
    /// collection stable. The first pass verifies actual JSON bytes by SHA-256;
    /// subsequent CREATE INDEX passes can reuse those keys by record identity.
    pub(crate) fn freeze(&self) -> Result<()> {
        if let Some(build) = self
            .building
            .lock()
            .map_err(|_| Error::Cache("index build cache poisoned".into()))?
            .as_mut()
        {
            build.rows = Some(HashMap::new());
        }
        Ok(())
    }
}
fn document_key(doc: Option<&Value>, field: &FieldOperand) -> String {
    let selected = doc.map(|d| field.select(d)).unwrap_or_default();
    match selected.as_slice() {
        [] => "E".into(),
        [v] => key(v),
        _ => "M".into(),
    }
}
fn prepared<T>(
    ctx: &rusqlite::functions::Context<'_>,
    building: &Building,
    wanted: &FieldOperand,
    kind: Option<ValueType>,
    convert: impl FnOnce(&crate::query::Scalar, &str) -> T,
) -> rusqlite::Result<Option<T>> {
    let mut building = building.lock().map_err(super::sql::failure)?;
    let Some(build) = building.as_mut() else {
        return Ok(None);
    };
    let Some(index) = build
        .fields
        .iter()
        .position(|(f, definition)| f == wanted && kind.is_none_or(|k| k == definition.kind))
    else {
        return Ok(None);
    };
    let id = (ctx.len() == 3 && build.rows.is_some())
        .then(|| ctx.get_raw(1).as_str())
        .transpose()?;
    if let Some(keys) = id.and_then(|id| build.rows.as_ref()?.get(id)) {
        return Ok(Some(convert(&keys[index].0, &keys[index].1)));
    }
    let bytes = ctx.get_raw(0).as_bytes()?;
    if let Some((previous, keys)) = &build.last
        && previous == bytes
    {
        return Ok(Some(convert(&keys[index].0, &keys[index].1)));
    }
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    let keys = if let Some(keys) = build.entries.get(&digest) {
        keys.clone()
    } else {
        let doc = std::str::from_utf8(bytes).ok().and_then(|s| {
            crate::json::index_fields(s, |name| {
                build
                    .fields
                    .iter()
                    .any(|(f, _)| matches!(&f.path[0],JsonPathStep::Key(key) if key==name))
            })
            .ok()
        });
        let keys = build.extract(doc.as_ref());
        build.remember(digest, &keys);
        keys
    };
    let result = convert(&keys[index].0, &keys[index].1);
    if let (Some(id), Some(rows)) = (id, build.rows.as_mut()) {
        let charge = 96 + id.len();
        // Only retain a row reference if the key vector is already budgeted.
        if build.entries.contains_key(&digest) && charge <= build.budget.saturating_sub(build.used)
        {
            build.used += charge;
            rows.insert(id.to_owned(), keys.clone());
        }
    }
    build.last = Some((bytes.to_vec(), keys));
    Ok(Some(result))
}
fn document(
    ctx: &rusqlite::functions::Context<'_>,
    cache: &DocumentCache,
    field: Option<&str>,
) -> rusqlite::Result<Option<Arc<Value>>> {
    let bytes = ctx.get_raw(0).as_bytes()?;
    let mut cache = cache.lock().map_err(super::sql::failure)?;
    if let Some((previous, previous_field, doc)) = &*cache
        && previous == bytes
        && (previous_field.is_none() || previous_field.as_deref() == field)
    {
        return Ok(doc.clone());
    }
    let doc = std::str::from_utf8(bytes)
        .ok()
        .and_then(|s| match field {
            Some(field) => crate::json::index_field(s, field).ok(),
            None => crate::json::parse(s).ok(),
        })
        .map(Arc::new);
    *cache = Some((bytes.to_vec(), field.map(str::to_owned), doc.clone()));
    Ok(doc)
}
pub(crate) fn register(db: &Connection) -> Result<IndexCache> {
    let flags = FunctionFlags::SQLITE_UTF8
        | FunctionFlags::SQLITE_DETERMINISTIC
        | FunctionFlags::SQLITE_INNOCUOUS;
    db.create_collation("objekte_json_v1", key_cmp)?;
    let cache: DocumentCache = Arc::new(Mutex::new(None));
    let building: Building = Arc::new(Mutex::new(None));
    let specs = Arc::new(Mutex::new(
        HashMap::<String, Arc<(FieldOperand, ValueType)>>::new(),
    ));
    for arity in [2, 3] {
        let key_building = building.clone();
        let keys = cache.clone();
        db.create_scalar_function("objekte_json_key", arity, flags, move |ctx| {
            let field =
                ctx.get_or_create_aux(arity - 1, |v| -> rusqlite::Result<FieldOperand> {
                    serde_json::from_str(v.as_str()?).map_err(super::sql::failure)
                })?;
            if let Some(key) = prepared(ctx, &key_building, &field, None, |_, key| key.to_owned())?
            {
                return Ok(key);
            }
            let top = match field.path.first() {
                Some(JsonPathStep::Key(k)) => Some(k.as_str()),
                _ => None,
            };
            let doc = document(ctx, &keys, top)?;
            Ok(document_key(doc.as_deref(), &field))
        })?;
        for state in [true, false] {
            let cache = cache.clone();
            let building = building.clone();
            let specs = specs.clone();
            db.create_scalar_function(
                if state {
                    "objekte_view_state"
                } else {
                    "objekte_view_value"
                },
                arity,
                flags,
                move |ctx| {
                    let spec = {
                        let text = ctx.get_raw(arity as usize - 1).as_str()?;
                        let mut specs = specs.lock().map_err(super::sql::failure)?;
                        if let Some(spec) = specs.get(text) {
                            spec.clone()
                        } else {
                            let (name, kind): (String, ValueType) =
                                serde_json::from_str(text).map_err(super::sql::failure)?;
                            let spec = Arc::new((field(&name), kind));
                            if specs.len() < 256 {
                                specs.insert(text.to_owned(), spec.clone());
                            }
                            spec
                        }
                    };
                    let convert = |value: &crate::query::Scalar, _: &str| {
                        if state {
                            rusqlite::types::Value::Integer(value.state())
                        } else {
                            value.sql()
                        }
                    };
                    if let Some(value) = prepared(ctx, &building, &spec.0, Some(spec.1), convert)? {
                        return Ok(value);
                    }
                    let JsonPathStep::Key(name) = &spec.0.path[0] else {
                        unreachable!()
                    };
                    let doc = document(ctx, &cache, Some(name))?;
                    let field = Field {
                        kind: spec.1,
                        nullable: true,
                        properties: BTreeMap::new(),
                        items: None,
                        relation: None,
                    };
                    let value = crate::query::scalar(
                        "",
                        doc.as_deref().unwrap_or(&Value::Null),
                        name,
                        Some(&field),
                    );
                    Ok(if state {
                        rusqlite::types::Value::Integer(value.state())
                    } else {
                        value.sql()
                    })
                },
            )?;
        }
    }
    Ok(IndexCache {
        document: cache,
        building,
    })
}

impl JsonFilter {
    pub(crate) fn ensure_query_indexes(
        &self,
        db: &Connection,
        c: &Collection,
        metrics: &mut Metrics,
    ) -> Result<()> {
        let start = std::time::Instant::now();
        let result = (|| -> Result<()> {
            for f in &self.scalar_paths {
                if !indexed(c, f) {
                    continue;
                }
                let JsonPathStep::Key(name) = &f.path[0] else {
                    unreachable!()
                };
                let index = crate::store::scalar_query_index(c, name);
                let exists: bool = db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name=?1)",
                    [&index],
                    |r| r.get(0),
                )?;
                if exists {
                    continue;
                }
                let table = crate::store::record_table(&c.name);
                let key = key_expr("", f);
                let scope = quote(&c.name);
                let tx = db.unchecked_transaction()?;
                tx.execute_batch(&format!("CREATE INDEX {index} ON {table}({key},id) WHERE collection={scope}; ANALYZE {index}; ANALYZE sqlite_autoindex_{table}_1"))?;
                tx.execute(
                    "UPDATE collections SET index_hash=?2 WHERE name=?1",
                    rusqlite::params![c.name, c.index_hash],
                )?;
                tx.commit()?;
                metrics.index_builds += 1;
                metrics.index_rows_written += db.query_row(
                    &format!("SELECT count(*) FROM {table} WHERE collection=?1"),
                    [&c.name],
                    |r| r.get::<_, i64>(0),
                )? as u64;
            }
            Ok(())
        })();
        metrics.index_nanos += start.elapsed().as_nanos() as u64;
        result
    }
    /// Check scalar cardinality over the entire scope before pagination or boolean
    /// short circuiting. Indexed paths check the multi-value sentinel by index seek.
    pub(crate) fn check_sql_cardinality(&self, db: &Connection, c: &Collection) -> Result<()> {
        let table = crate::store::record_table(&c.name);
        let mut remaining = Vec::new();
        for f in &self.scalar_paths {
            if indexed(c, f) {
                let JsonPathStep::Key(name) = &f.path[0] else {
                    unreachable!()
                };
                let index = crate::store::scalar_query_index(c, name);
                let sql = format!(
                    "SELECT EXISTS(SELECT 1 FROM {table} r INDEXED BY {index} WHERE collection={} AND {}='M')",
                    quote(&c.name),
                    key_expr("r", f)
                );
                if db.query_row(&sql, [], |r| r.get::<_, bool>(0))? {
                    return Err(Error::Request(format!(
                        "path {} yields multiple values; use a '?' comparison or :each",
                        serde_json::to_string(&f.path).unwrap()
                    )));
                }
            } else {
                remaining.push(f.clone());
            }
        }
        if !remaining.is_empty() {
            db.query_row(&format!("SELECT count(*) FROM {table} r CROSS JOIN record_validation h USING(collection,id) WHERE r.collection=?1 AND h.parse_status='parsed' AND objekte_check_paths(r.json,r.digest,?2)"), rusqlite::params![c.name,serde_json::to_string(&remaining).unwrap()], |r| r.get::<_,i64>(0))?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "indexes.tests.rs"]
mod tests;
