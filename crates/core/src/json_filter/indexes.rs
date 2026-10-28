//! SQLite expression indexes over raw JSON. No materialized field columns.
use super::*;
use crate::{
    Collection,
    schema::{Field, ValueType},
};
use rusqlite::{Connection, functions::FunctionFlags};
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
    format!(
        "objekte_json_key({json},{}) COLLATE objekte_json_v1",
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
    format!(
        "objekte_view_{}({json},{})",
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
fn document(
    ctx: &rusqlite::functions::Context<'_>,
    cache: &DocumentCache,
    field: Option<&str>,
) -> rusqlite::Result<Option<Arc<Value>>> {
    let bytes = ctx.get_raw(0).as_bytes()?;
    let mut cache = cache.lock().map_err(super::sql::failure)?;
    if let Some((previous, previous_field, doc)) = &*cache
        && previous == bytes
        && previous_field.as_deref() == field
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
pub(crate) fn register(db: &Connection) -> Result<()> {
    let flags = FunctionFlags::SQLITE_UTF8
        | FunctionFlags::SQLITE_DETERMINISTIC
        | FunctionFlags::SQLITE_INNOCUOUS;
    db.create_collation("objekte_json_v1", key_cmp)?;
    let cache: DocumentCache = Arc::new(Mutex::new(None));
    let keys = cache.clone();
    db.create_scalar_function("objekte_json_key", 2, flags, move |ctx| {
        let field = ctx.get_or_create_aux(1, |v| -> rusqlite::Result<FieldOperand> {
            serde_json::from_str(v.as_str()?).map_err(super::sql::failure)
        })?;
        let top = match field.path.first() {
            Some(JsonPathStep::Key(k)) => Some(k.as_str()),
            _ => None,
        };
        let doc = document(ctx, &keys, top)?;
        let selected = doc.as_ref().map(|d| field.select(d)).unwrap_or_default();
        Ok(match selected.as_slice() {
            [] => "E".into(),
            [v] => key(v),
            _ => "M".into(),
        })
    })?;
    for state in [true, false] {
        let cache = cache.clone();
        db.create_scalar_function(
            if state {
                "objekte_view_state"
            } else {
                "objekte_view_value"
            },
            2,
            flags,
            move |ctx| {
                let spec =
                    ctx.get_or_create_aux(1, |v| -> rusqlite::Result<(String, ValueType)> {
                        serde_json::from_str(v.as_str()?).map_err(super::sql::failure)
                    })?;
                let doc = document(ctx, &cache, Some(&spec.0))?;
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
                    &spec.0,
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
    Ok(())
}

impl JsonFilter {
    pub(crate) fn uses_indexes(&self, c: &Collection) -> bool {
        self.paths.iter().any(|p| {
            indexed(
                c,
                &FieldOperand {
                    path: p.clone(),
                    modifier: None,
                },
            )
        })
    }
    /// Check scalar cardinality over the entire scope before pagination or boolean
    /// short circuiting. Indexed paths check the multi-value sentinel by index seek.
    pub(crate) fn check_sql_cardinality(&self, db: &Connection, c: &Collection) -> Result<()> {
        let mut remaining = Vec::new();
        for f in &self.scalar_paths {
            if indexed(c, f) {
                let sql = format!(
                    "SELECT EXISTS(SELECT 1 FROM records r WHERE collection={} AND {}='M')",
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
            db.query_row("SELECT count(*) FROM records r JOIN record_validation h USING(collection,id) WHERE r.collection=?1 AND h.parse_status='parsed' AND objekte_check_paths(r.json,r.digest,?2)", rusqlite::params![c.name,serde_json::to_string(&remaining).unwrap()], |r| r.get::<_,i64>(0))?;
        }
        Ok(())
    }
}
