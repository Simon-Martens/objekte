//! Configured SQLite indexes for views and scalar queries.

use super::*;

fn index_prefix(c: &Collection) -> String {
    format!("j_{}", digest(c.name.as_bytes()))
}

pub(crate) fn scalar_query_index(c: &Collection, field: &str) -> String {
    let i = c.indexes.iter().position(|f| f.name == field).unwrap();
    format!("{}_{i}_query", index_prefix(c))
}

pub(crate) fn ordered_view_index(c: &Collection, sorts: &[crate::Sort]) -> Option<String> {
    let prefix = index_prefix(c);
    match sorts {
        [id] if id.field == "@id" => Some(format!("sqlite_autoindex_{}_1", record_table())),
        [field, id] if id.field == "@id" && id.direction == crate::Direction::Asc => {
            let i = c.indexes.iter().position(|f| f.name == field.field)?;
            let direction = if field.direction == crate::Direction::Asc {
                "asc"
            } else {
                "desc"
            };
            Some(format!("{prefix}_{i}_{direction}"))
        }
        _ => None,
    }
}


impl Store {
    pub fn has_indexes(&self, c: &Collection) -> Result<bool> {
        Ok(self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name GLOB ?1)",
            [format!("{}_*", index_prefix(c))],
            |r| r.get(0),
        )?)
    }

    pub fn has_requested_indexes(&self, names: &[String]) -> Result<bool> {
        for name in names {
            if !index_exists(&self.db, name)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}


pub(crate) fn drop_indexes(db: &Connection, c: &Collection) -> Result<()> {
    let prefix = index_prefix(c);
    let mut stmt =
        db.prepare("SELECT name FROM sqlite_master WHERE type='index' AND name GLOB ?1")?;
    let names = stmt
        .query_map([format!("{prefix}_*")], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(stmt);
    for name in names {
        if name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            db.execute_batch(&format!("DROP INDEX {name}"))?;
        }
    }
    Ok(())
}

pub(crate) fn create_indexes(
    db: &Connection,
    c: &Collection,
    requested: &[String],
    metrics: &mut Metrics,
) -> Result<()> {
    let table = record_table();
    use crate::json_filter::indexes::{quote, view_expr};
    let prefix = index_prefix(c);
    let scope = format!(" WHERE collection={}", quote(&c.name));
    // PERFORMANCE: One compact covering index supplies all configured view keys for compound
    // orders/filters without reparsing full JSON or adding stored table columns.
    let fields = c
        .indexes
        .iter()
        .filter(|f| f.name != "@id")
        .map(|f| {
            format!(
                ",{},{}",
                view_expr(c, &f.name, "", true),
                view_expr(c, &f.name, "", false)
            )
        })
        .collect::<String>();
    if requested.contains(&format!("{prefix}_identity"))
        && !index_exists(db, &format!("{prefix}_identity"))?
    {
        db.execute_batch(&format!(
            "CREATE INDEX {prefix}_identity ON {table}(id{fields}){scope}"
        ))?;
        metrics.index_builds += 1;
    }
    for (i, f) in c.indexes.iter().enumerate() {
        if f.name == "@id" {
            continue;
        }
        let state = view_expr(c, &f.name, "", true);
        let value = view_expr(c, &f.name, "", false);
        for (suffix, direction) in [("asc", "ASC"), ("desc", "DESC")] {
            if !requested.contains(&format!("{prefix}_{i}_{suffix}"))
                || index_exists(db, &format!("{prefix}_{i}_{suffix}"))?
            {
                continue;
            }
            db.execute_batch(&format!("CREATE INDEX {prefix}_{i}_{suffix} ON {table}({state} ASC,{value} {direction},id ASC){scope}"))?;
            metrics.index_builds += 1;
        }
    }
    // INFO: Count source records indexed, not separate materialized field-table rows.
    metrics.index_rows_written += db.query_row(
        &format!("SELECT count(*) FROM {table} WHERE collection=?1"),
        [&c.name],
        |r| r.get::<_, i64>(0),
    )? as u64;
    Ok(())
}

pub(crate) fn analyze_indexes(db: &Connection, requested: &[String]) -> Result<()> {
    for name in requested {
        db.execute_batch(&format!("ANALYZE {name}"))?;
    }
    db.execute_batch(&format!(
        "ANALYZE sqlite_autoindex_{}_1",
        record_table()
    ))?;
    Ok(())
}

fn index_exists(db: &Connection, name: &str) -> Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name=?1)",
        [name],
        |r| r.get(0),
    )?)
}

pub(crate) fn page_view_index(c: &Collection, plan: &crate::query::Plan) -> String {
    if plan.filters.is_empty() {
        ordered_view_index(c, &plan.sorts)
    } else {
        None
    }
    .unwrap_or_else(|| covering_view_index(c))
}

pub(crate) fn requested_view_indexes(c: &Collection, plan: &crate::query::Plan) -> Vec<String> {
    let mut names = vec![page_view_index(c, plan)];
    if plan.count {
        let count = filter_view_index(c, &plan.filters);
        if !names.contains(&count) {
            names.push(count);
        }
    }
    names.retain(|name| name.starts_with("j_"));
    names
}

pub(crate) fn requested_view_fields(c: &Collection, names: &[String]) -> BTreeSet<String> {
    let prefix = index_prefix(c);
    c.indexes
        .iter()
        .enumerate()
        .filter(|(i, f)| {
            f.name != "@id"
                && (names.contains(&covering_view_index(c))
                    || ["asc", "desc"]
                        .iter()
                        .any(|suffix| names.contains(&format!("{prefix}_{i}_{suffix}"))))
        })
        .map(|(_, f)| f.name.clone())
        .collect()
}

pub(crate) fn covering_view_index(c: &Collection) -> String {
    format!("{}_identity", index_prefix(c))
}

pub(crate) fn filter_view_index(c: &Collection, filters: &[crate::Filter]) -> String {
    if filters.is_empty() || filters.iter().all(|f| f.field == "@id") {
        return format!("sqlite_autoindex_{}_1", record_table());
    }
    if let Some(first) = filters.first()
        && first.field != "@id"
        && filters.iter().all(|f| f.field == first.field)
    {
        let i = c
            .indexes
            .iter()
            .position(|f| f.name == first.field)
            .unwrap();
        return format!("{}_{i}_asc", index_prefix(c));
    }
    covering_view_index(c)
}
