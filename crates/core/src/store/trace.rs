//! Opt-in per-statement SQL timing for CLI diagnostics.

use super::*;

pub(super) fn install_sql_trace(db: &mut Connection, active: Arc<AtomicBool>) -> Result<()> {
    // INFO: SQLite invokes this callback while executing a statement. It must not
    // run SQL on the same connection; writing one JSON line to stderr is safe.
    let mut started: HashMap<String, Instant> = HashMap::new();
    db.trace_v2(
        TraceEventCodes::SQLITE_TRACE_STMT | TraceEventCodes::SQLITE_TRACE_PROFILE,
        Some(move |event: TraceEvent<'_>| {
            if !active.load(Ordering::Relaxed) {
                return;
            }
            match event {
                TraceEvent::Stmt(statement, _) => {
                    started
                        .entry(statement.sql().into_owned())
                        .or_insert_with(Instant::now);
                }
                TraceEvent::Profile(statement, sqlite_duration) => {
                    let sql = statement.sql().into_owned();
                    let elapsed = started
                        .remove(&sql)
                        .map_or(sqlite_duration, |start| start.elapsed());
                    let mut stderr = std::io::stderr().lock();
                    let _ = writeln!(
                        stderr,
                        "{}",
                        serde_json::json!({
                            "type": "sql",
                            "sql": sql,
                            "duration_ms": elapsed.as_secs_f64() * 1000.0,
                        })
                    );
                }
                _ => {}
            }
        }),
    )?;
    Ok(())
}
