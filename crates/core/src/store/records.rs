//! Transactional cache record and relation updates.

use super::*;

pub(crate) fn put_edges(
    db: &Connection,
    c: &str,
    id: &str,
    edges: &[RecordRelation],
    metrics: &mut Metrics,
) -> Result<()> {
    let t = Instant::now();
    let mut stmt=db.prepare_cached("SELECT location,target_collection,target_id FROM edges WHERE source_collection=?1 AND source_id=?2")?;
    let old = stmt
        .query_map(params![c, id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<std::result::Result<BTreeSet<_>, _>>()?;
    let new = edges
        .iter()
        .map(|e| {
            (
                e.location.clone(),
                e.target_collection.clone(),
                e.target_id.clone(),
            )
        })
        .collect::<BTreeSet<_>>();
    for (location, target, target_id) in old.difference(&new) {
        db.prepare_cached("DELETE FROM edges WHERE source_collection=?1 AND source_id=?2 AND location=?3 AND target_collection=?4 AND target_id=?5")?.execute(params![c,id,location,target,target_id])?;
        metrics.edges_removed += 1;
    }
    for (location, target, target_id) in new.difference(&old) {
        db.prepare_cached("INSERT INTO edges VALUES(?1,?2,?3,?4,?5)")?
            .execute(params![c, id, location, target, target_id])?;
        metrics.edges_added += 1;
    }
    metrics.relation_write_nanos += t.elapsed().as_nanos() as u64;
    Ok(())
}

pub(crate) fn delete_record(
    db: &Connection,
    c: &Collection,
    id: &str,
    metrics: &mut Metrics,
) -> Result<()> {
    db.prepare_cached("DELETE FROM record_validation WHERE collection=?1 AND id=?2")?
        .execute(params![c.name, id])?;
    metrics.rows_deleted += db
        .prepare_cached(&format!(
            "DELETE FROM {} WHERE collection=?1 AND id=?2",
            record_table(&c.name)
        ))?
        .execute(params![c.name, id])? as u64;
    metrics.edges_removed += db
        .prepare_cached("DELETE FROM edges WHERE source_collection=?1 AND source_id=?2")?
        .execute(params![c.name, id])? as u64;
    Ok(())
}
