//! Workers used only while projecting source files into a collection index.

use crate::{Error, Result};

pub(super) fn run<T: Send>(work: impl FnOnce() -> Result<T> + Send) -> Result<T> {
    std::thread::scope(|scope| {
        let mut workers = Vec::new();
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(rayon::current_num_threads())
            .spawn_handler(|thread| {
                let mut builder = std::thread::Builder::new();
                if let Some(name) = thread.name() {
                    builder = builder.name(name.to_owned());
                }
                if let Some(size) = thread.stack_size() {
                    builder = builder.stack_size(size);
                }
                workers.push(builder.spawn_scoped(scope, move || thread.run())?);
                Ok(())
            })
            .build()
            .map_err(|error| Error::Cache(format!("cannot start index scan workers: {error}")))?;

        let result = pool.install(work);
        // Sonic's parser and the bigram builder keep thread-local scratch buffers.
        // Dropping Rayon alone requests shutdown. Explicit joins wait for TLS
        // destructors as well, before the ready index reaches its caller.
        drop(pool);
        for worker in workers {
            worker.join().expect("index scan worker panicked");
        }
        result
    })
}

#[cfg(test)]
#[path = "scan_pool.test.rs"]
mod tests;
