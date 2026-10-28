use super::*;
use rusqlite::{
    Connection,
    functions::{Context, FunctionFlags},
};
use std::sync::{Arc, Mutex};

type CachedDocument = Arc<Mutex<Option<(String, Arc<Value>)>>>;
fn failure(error: impl std::fmt::Display) -> rusqlite::Error {
    rusqlite::Error::UserFunctionError(Box::new(std::io::Error::other(error.to_string())))
}
fn document(ctx: &Context<'_>, cache: &CachedDocument) -> rusqlite::Result<Arc<Value>> {
    let hash = ctx.get_raw(1).as_str()?;
    let mut cached = cache.lock().map_err(failure)?;
    if let Some((previous, value)) = cached.as_ref()
        && previous == hash
    {
        return Ok(value.clone());
    }
    let bytes = ctx.get_raw(0).as_bytes()?;
    let text = std::str::from_utf8(bytes).map_err(failure)?;
    let value = Arc::new(crate::json::parse(text).map_err(failure)?);
    *cached = Some((hash.into(), value.clone()));
    Ok(value)
}

/// Installed for each SQL query. At most one decoded document is shared between
/// path functions while scanning; no persistent JSON copy or result cache is built.
pub(crate) fn register(db: &Connection) -> Result<()> {
    let flags = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;
    let cache: CachedDocument = Arc::new(Mutex::new(None));
    let checks = cache.clone();
    db.create_scalar_function("objekte_check_paths", 3, flags, move |ctx| {
        let doc = document(ctx, &checks)?;
        let paths = ctx.get_or_create_aux(2, |v| -> rusqlite::Result<Vec<JsonPath>> {
            serde_json::from_str(v.as_str()?).map_err(failure)
        })?;
        check_paths(&doc, &paths).map_err(failure)?;
        Ok(true)
    })?;
    db.create_scalar_function("objekte_values", 4, flags, move |ctx| {
        let doc = document(ctx, &cache)?;
        let path = ctx.get_or_create_aux(2, |v| -> rusqlite::Result<JsonPath> {
            serde_json::from_str(v.as_str()?).map_err(failure)
        })?;
        let flatten = ctx.get::<bool>(3)?;
        let selected = values(&doc, &path, flatten)
            .into_iter()
            .map(|v| {
                // Keep number lexemes as text and boolean tags separate from integers.
                // SQLite's numeric JSON extraction would round some valid JSON values.
                let value = match v {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    Value::Bool(b) => b.to_string(),
                    _ => String::new(),
                };
                serde_json::json!({"kind":kind(v), "value":value})
            })
            .collect::<Vec<_>>();
        serde_json::to_string(&selected).map_err(failure)
    })?;
    db.create_scalar_function("objekte_decimal_cmp", 2, flags, |ctx| {
        let a = ctx.get_raw(0).as_str()?;
        let b = ctx.get_raw(1).as_str()?;
        // Only validated number lexemes reach this function from generated SQL.
        Ok(match decimal_cmp(a, b) {
            Ordering::Less => -1,
            Ordering::Equal => 0,
            Ordering::Greater => 1,
        })
    })?;
    db.create_scalar_function("objekte_contains", 2, flags, |ctx| {
        let needle = ctx.get_or_create_aux(1, |v| -> rusqlite::Result<String> {
            Ok(caseless::default_case_fold_str(v.as_str()?))
        })?;
        Ok(caseless::default_case_fold_str(ctx.get_raw(0).as_str()?).contains(needle.as_str()))
    })?;
    Ok(())
}

impl JsonFilter {
    /// The predicate is compiled over aliases r (records) and h (record_validation).
    /// Values and paths are always bindings; no filter text becomes SQL syntax.
    pub(crate) fn sql(&self) -> (String, Vec<String>) {
        fn bind(bindings: &mut Vec<String>, value: String) -> String {
            bindings.push(value);
            format!("?{}", bindings.len())
        }
        fn expr(e: &Expr, bindings: &mut Vec<String>) -> String {
            match e {
                Expr::And(es) | Expr::Or(es) => {
                    let join = if matches!(e, Expr::And(_)) {
                        " AND "
                    } else {
                        " OR "
                    };
                    format!(
                        "({})",
                        es.iter()
                            .map(|e| expr(e, bindings))
                            .collect::<Vec<_>>()
                            .join(join)
                    )
                }
                Expr::Exists(path) => {
                    let p = bind(bindings, serde_json::to_string(path).unwrap());
                    format!(
                        "EXISTS (SELECT 1 FROM json_each(objekte_values(r.json,r.digest,{p},0)))"
                    )
                }
                Expr::Compare {
                    path, op, value, ..
                } => {
                    let p = bind(bindings, serde_json::to_string(path).unwrap());
                    let rhs_kind = kind(value);
                    let rhs = bind(
                        bindings,
                        match value {
                            Value::String(s) => s.clone(),
                            _ => value.to_string(),
                        },
                    );
                    let k = "json_extract(f.value,'$.kind')";
                    let v = "json_extract(f.value,'$.value')";
                    let cmp = if rhs_kind == "number" {
                        format!("objekte_decimal_cmp({v},{rhs})")
                    } else {
                        String::new()
                    };
                    let eq = match rhs_kind {
                        "null" => format!("{rhs}='null'"),
                        "number" => format!("{cmp}=0"),
                        _ => format!("{v}={rhs} COLLATE BINARY"),
                    };
                    let equal = format!("CASE WHEN {k}='{rhs_kind}' THEN {eq} ELSE 0 END");
                    let predicate = match op {
                        Op::Eq => equal,
                        Op::Ne => format!("{k}<>'container' AND NOT ({equal})"),
                        Op::Contains | Op::NotContains => {
                            let cmp = if matches!(op, Op::Contains) {
                                "<>"
                            } else {
                                "="
                            };
                            format!(
                                "CASE WHEN {k}='string' THEN objekte_contains({v},{rhs}){cmp}0 ELSE 0 END"
                            )
                        }
                        _ if matches!(rhs_kind, "number" | "string") => {
                            let op = match op {
                                Op::Lt => "<",
                                Op::Le => "<=",
                                Op::Gt => ">",
                                Op::Ge => ">=",
                                _ => unreachable!(),
                            };
                            let comparison = if rhs_kind == "number" {
                                format!("{cmp}{op}0")
                            } else {
                                format!("{v}{op}{rhs} COLLATE BINARY")
                            };
                            format!("CASE WHEN {k}='{rhs_kind}' THEN {comparison} ELSE 0 END")
                        }
                        _ => format!("CASE WHEN {rhs} IS NULL THEN 0 ELSE 0 END"),
                    };
                    format!(
                        "EXISTS (SELECT 1 FROM json_each(objekte_values(r.json,r.digest,{p},1)) f WHERE {predicate})"
                    )
                }
            }
        }
        let mut bindings = vec![serde_json::to_string(&self.scalar_paths).unwrap()];
        let predicate = expr(&self.expr, &mut bindings);
        (
            format!(
                "CASE WHEN h.parse_status='parsed' THEN CASE WHEN objekte_check_paths(r.json,r.digest,?1) THEN {predicate} ELSE 0 END ELSE 0 END"
            ),
            bindings,
        )
    }
}
