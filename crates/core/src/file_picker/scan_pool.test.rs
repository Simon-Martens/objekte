use super::run;
use rayon::prelude::*;
use std::sync::atomic::{AtomicUsize, Ordering};

static WORKER_DROPS: AtomicUsize = AtomicUsize::new(0);
static WORKER_STARTS: AtomicUsize = AtomicUsize::new(0);


/// Counts the end of a scan worker's thread-local lifetime.
struct WorkerMarker;


impl Drop for WorkerMarker {
    fn drop(&mut self) {
        WORKER_DROPS.fetch_add(1, Ordering::SeqCst);
    }
}


thread_local! {
    static WORKER_MARKER: WorkerMarker = {
        WORKER_STARTS.fetch_add(1, Ordering::SeqCst);
        WorkerMarker
    };
}

#[test]
fn joins_workers_and_drops_thread_local_buffers_before_returning() {
    let starts_before = WORKER_STARTS.load(Ordering::SeqCst);
    let drops_before = WORKER_DROPS.load(Ordering::SeqCst);
    run(|| {
        (0..4096).into_par_iter().for_each(|_| {
            assert!(rayon::current_thread_index().is_some());
            WORKER_MARKER.with(|_| {});
        });
        Ok(())
    })
    .unwrap();

    let started = WORKER_STARTS.load(Ordering::SeqCst) - starts_before;
    assert!(started > 0);
    assert_eq!(WORKER_DROPS.load(Ordering::SeqCst) - drops_before, started);
}
