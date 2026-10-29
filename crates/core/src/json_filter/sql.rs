use super::*;
use rusqlite::{
    Connection,
    functions::{Context, FunctionFlags},
};
use std::sync::{Arc, Mutex};

type CachedDocument = Arc<Mutex<Option<(Vec<u8>, Option<Arc<Value>>)>>>;
pub(super) fn failure(error: impl std::fmt::Display) -> rusqlite::Error {
    rusqlite::Error::UserFunctionError(Box::new(std::io::Error::other(error.to_string())))
}

fn document(ctx: &Context<'_>, cache: &CachedDocument) -> rusqlite::Result<Option<Arc<Value>>> {
    let bytes = ctx.get_raw(0).as_bytes()?;
    let mut cached = cache.lock().map_err(failure)?;
    if let Some((previous, value)) = cached.as_ref()
        && previous == bytes
    {
        return Ok(value.clone());
    }
    let value = std::str::from_utf8(bytes)
        .ok()
        .and_then(|s| crate::json::parse(s).ok())
        .map(Arc::new);
    *cached = Some((bytes.to_vec(), value.clone()));
    Ok(value)
}

fn envelope(v: &Value) -> Value {
    let text = match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => String::new(),
    };
    serde_json::json!({"kind":kind(v), "value":text})
}

pub(crate) fn register(db: &Connection) -> Result<()> {
    let flags = FunctionFlags::SQLITE_UTF8
        | FunctionFlags::SQLITE_DETERMINISTIC
        | FunctionFlags::SQLITE_INNOCUOUS;
    let cache: CachedDocument = Arc::new(Mutex::new(None));
    let checks = cache.clone();
    db.create_scalar_function("objekte_check_paths", 3, flags, move |ctx| {
        let doc = document(ctx, &checks)?;
        let paths = ctx.get_or_create_aux(2, |v| -> rusqlite::Result<Vec<FieldOperand>> {
            serde_json::from_str(v.as_str()?).map_err(failure)
        })?;
        if let Some(doc) = doc {
            check_paths(&doc, &paths).map_err(failure)?;
        }
        Ok(true)
    })?;
    db.create_scalar_function("objekte_values", 4, flags, move |ctx| {
        let doc = document(ctx, &cache)?;
        let operand = ctx.get_or_create_aux(2, |v| -> rusqlite::Result<Operand> {
            serde_json::from_str(v.as_str()?).map_err(failure)
        })?;
        let flatten = ctx.get::<bool>(3)?;
        let selected = doc
            .map(|doc| {
                if !flatten && let Operand::Field(f) = &*operand {
                    values(&doc, &f.path, false)
                        .into_iter()
                        .map(envelope)
                        .collect::<Vec<_>>()
                } else {
                    operand.select(&doc).iter().map(envelope).collect()
                }
            })
            .unwrap_or_default();
        serde_json::to_string(&selected).map_err(failure)
    })?;
    db.create_scalar_function("objekte_decimal_cmp", 2, flags, |ctx| {
        Ok(
            match decimal_cmp(ctx.get_raw(0).as_str()?, ctx.get_raw(1).as_str()?) {
                Ordering::Less => -1,
                Ordering::Equal => 0,
                Ordering::Greater => 1,
            },
        )
    })?;
    db.create_scalar_function("objekte_like", 3, flags, |ctx| {
        let field = ctx.get::<bool>(2)?;
        let pattern = ctx.get_or_create_aux(1, |v| -> rusqlite::Result<pattern::Like> {
            Ok(pattern::Like::new(v.as_str()?, field))
        })?;
        Ok(pattern.matches(ctx.get_raw(0).as_str()?))
    })?;
    Ok(())
}

fn bind(bindings: &mut Vec<String>, value: String) -> String {
    if let Some(index) = bindings.iter().position(|bound| bound == &value) {
        return format!("?{}", index + 1);
    }
    bindings.push(value);
    format!("?{}", bindings.len())
}

fn selected(operand: &Operand, flatten: bool, bindings: &mut Vec<String>) -> String {
    match operand {
        Operand::Literal(v) => {
            let e = envelope(v);
            let value = bind(bindings, e["value"].as_str().unwrap().to_owned());
            format!(
                "json_array(json_object('kind','{}','value',{value}))",
                kind(v)
            )
        }
        Operand::Field(_) => {
            let p = bind(bindings, serde_json::to_string(operand).unwrap());
            format!("objekte_values(r.json,r.digest,{p},{})", u8::from(flatten))
        }
    }
}

fn indexed_comparison(
    left: &Operand,
    op: Op,
    right: &Operand,
    c: &crate::Collection,
    bindings: &mut Vec<String>,
) -> Option<String> {
    let (field, op, value) = match (left, right) {
        (Operand::Field(f), Operand::Literal(v)) => (f, op, v),
        (Operand::Literal(v), Operand::Field(f)) => (
            f,
            match op {
                Op::Lt => Op::Gt,
                Op::Le => Op::Ge,
                Op::Gt => Op::Lt,
                Op::Ge => Op::Le,
                op => op,
            },
            v,
        ),
        _ => return None,
    };
    if !indexes::indexed(c, field) || matches!(op, Op::Contains | Op::NotContains) {
        return None;
    }
    let key = indexes::key_expr("r", field);
    if matches!(op, Op::Eq) {
        return Some(format!("{key}={}", bind(bindings, indexes::key(value))));
    }
    if matches!(op, Op::Ne) {
        let rhs = bind(bindings, indexes::key(value));
        return Some(format!(
            "(({key}>='B' AND {key}<'C') OR ({key}>='N' AND {key}<'O') OR ({key}>='S' AND {key}<'T') OR {key}='Z') AND {key}<>{rhs}"
        ));
    }
    let (lower, upper) = match value {
        Value::Number(_) => ('N', 'O'),
        Value::String(_) => ('S', 'T'),
        _ => return Some("0".into()),
    };
    let op = match op {
        Op::Lt => "<",
        Op::Le => "<=",
        Op::Gt => ">",
        Op::Ge => ">=",
        _ => unreachable!(),
    };
    let rhs = bind(bindings, indexes::key(value));
    Some(format!(
        "{key}>='{lower}' AND {key}<'{upper}' AND {key}{op}{rhs}"
    ))
}


impl JsonFilter {
    /// Indexed paths are compiler-escaped SQL literals so SQLite can recognize
    /// expression indexes. All comparison values remain bound parameters.
    pub(crate) fn sql(&self, c: &crate::Collection, bindings: &mut Vec<String>) -> String {
        fn expr(e: &Expr, c: &crate::Collection, bindings: &mut Vec<String>) -> String {
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
                            .map(|e| format!("({})", expr(e, c, bindings)))
                            .collect::<Vec<_>>()
                            .join(join)
                    )
                }
                Expr::Exists(path) => {
                    let selection = selected(
                        &Operand::Field(FieldOperand {
                            path: path.clone(),
                            modifier: None,
                        }),
                        false,
                        bindings,
                    );
                    format!("EXISTS (SELECT 1 FROM json_each({selection}))")
                }
                Expr::Compare {
                    left,
                    op,
                    any,
                    right,
                } => {
                    if !any && let Some(sql) = indexed_comparison(left, *op, right, c, bindings) {
                        return sql;
                    }
                    let (ak, av, bk, bv, from) = match (left, right) {
                        (Operand::Field(_), Operand::Literal(value)) => {
                            let a = selected(left, true, bindings);
                            (
                                "json_extract(a.value,'$.kind')".into(),
                                "json_extract(a.value,'$.value')".into(),
                                format!("'{}'", kind(value)),
                                bind(bindings, envelope(value)["value"].as_str().unwrap().into()),
                                format!("json_each({a}) a"),
                            )
                        }
                        (Operand::Literal(value), Operand::Field(_)) => {
                            let b = selected(right, true, bindings);
                            (
                                format!("'{}'", kind(value)),
                                bind(bindings, envelope(value)["value"].as_str().unwrap().into()),
                                "json_extract(b.value,'$.kind')".into(),
                                "json_extract(b.value,'$.value')".into(),
                                format!("json_each({b}) b"),
                            )
                        }
                        _ => {
                            let a = selected(left, true, bindings);
                            let b = selected(right, true, bindings);
                            (
                                "json_extract(a.value,'$.kind')".into(),
                                "json_extract(a.value,'$.value')".into(),
                                "json_extract(b.value,'$.kind')".into(),
                                "json_extract(b.value,'$.value')".into(),
                                format!("json_each({a}) a CROSS JOIN json_each({b}) b"),
                            )
                        }
                    };
                    let equal = format!(
                        "CASE WHEN {ak}<>{bk} OR {ak}='container' THEN 0 WHEN {ak}='number' THEN objekte_decimal_cmp({av},{bv})=0 ELSE {av}={bv} COLLATE BINARY END"
                    );
                    let predicate = match op {
                        Op::Eq => equal,
                        Op::Ne => {
                            format!("{ak}<>'container' AND {bk}<>'container' AND NOT ({equal})")
                        }
                        Op::Contains | Op::NotContains => {
                            let yes = u8::from(matches!(op, Op::Contains));
                            let field = u8::from(matches!(right, Operand::Field(_)));
                            format!(
                                "CASE WHEN {ak}='string' AND {bk}='string' THEN objekte_like({av},{bv},{field})={yes} ELSE 0 END"
                            )
                        }
                        _ => {
                            let op = match op {
                                Op::Lt => "<",
                                Op::Le => "<=",
                                Op::Gt => ">",
                                Op::Ge => ">=",
                                _ => unreachable!(),
                            };
                            format!(
                                "CASE WHEN {ak}<>{bk} THEN 0 WHEN {ak}='number' THEN objekte_decimal_cmp({av},{bv}){op}0 WHEN {ak}='string' THEN {av}{op}{bv} COLLATE BINARY ELSE 0 END"
                            )
                        }
                    };
                    if !any && (left.each() || right.each()) {
                        format!("COALESCE((SELECT min({predicate}) FROM {from}),0)")
                    } else {
                        format!("EXISTS (SELECT 1 FROM {from} WHERE {predicate})")
                    }
                }
            }
        }
        expr(&self.expr, c, bindings)
    }
}
