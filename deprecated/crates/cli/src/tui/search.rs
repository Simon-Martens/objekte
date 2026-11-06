//! Background query execution and ordered hit events for the terminal UI.

use super::{
    app::{App, Hit, InferRequest, Overlay, PAGE_SIZE, Request},
    navigation::BacklinkStatus,
    relations,
};
use objekte_core::{Error, InferOptions, InferenceReport, Record, Repository};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
    thread,
};


/// One provisional row or the final query outcome and repository handle.
enum SearchUpdate {
    Hit(Box<Hit>),
    Page(Vec<Hit>),
    Backlink(Box<Record>),
    Inferred {
        repository: Box<Repository>,
        result: Result<InferenceReport, String>,
    },
    SchemaWritten {
        repository: Box<Repository>,
        report: InferenceReport,
        result: Result<usize, String>,
    },
    Finished {
        repository: Box<Repository>,
        total: Result<Option<usize>, String>,
        truncated: bool,
        elapsed_ms: Option<f64>,
    },
}


/// Which generation a repository-owning job belongs to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum JobKind {
    Search,
    Backlinks,
    Inference,
    SchemaWrite,
}


/// A query worker and its generation, allowing stale rows to be discarded.
struct Running {
    receiver: Receiver<SearchUpdate>,
    cancel: Arc<AtomicBool>,
    generation: u64,
    kind: JobKind,
}


impl Running {
    fn current(&self, app: &App<'_>) -> bool {
        self.generation
            == match self.kind {
                JobKind::Search => app.generation,
                JobKind::Backlinks => app.links.generation,
                JobKind::Inference => self.generation,
                JobKind::SchemaWrite => self.generation,
            }
    }
}


/// Drives one repository-owning worker at a time from the TUI event loop.
#[derive(Default)]
pub(super) struct SearchWorker {
    running: Option<Running>,
}


impl SearchWorker {
    pub(super) fn is_idle(&self) -> bool {
        self.running.is_none()
    }

    pub(super) fn drive(&mut self, app: &mut App<'_>) {
        if let Some(running) = &self.running
            && !running.current(app)
        {
            running.cancel.store(true, Ordering::Relaxed);
        }
        while let Some(running) = &self.running {
            let current = running.current(app);
            let kind = running.kind;
            match running.receiver.try_recv() {
                Ok(SearchUpdate::Hit(hit)) => {
                    if kind == JobKind::Search && current {
                        app.receive_hit(*hit);
                    }
                }
                Ok(SearchUpdate::Page(page)) => {
                    if kind == JobKind::Search && current {
                        app.receive_page(page);
                    }
                }
                Ok(SearchUpdate::Backlink(record)) => {
                    if kind == JobKind::Backlinks && current {
                        app.add_backlink_record(&record);
                    }
                }
                Ok(SearchUpdate::Inferred {
                    mut repository,
                    result,
                }) => {
                    std::mem::swap(app.repo, &mut *repository);
                    self.running = None;
                    app.index_status = None;
                    match result {
                        Ok(report) => {
                            app.last_inference = Some(report);
                            app.inference_saved = false;
                            app.overlay = Some(Overlay::InferReady);
                        }
                        Err(error) => app.error(format!("Inference failed: {error}")),
                    }
                    break;
                }
                Ok(SearchUpdate::SchemaWritten {
                    mut repository,
                    report,
                    result,
                }) => {
                    std::mem::swap(app.repo, &mut *repository);
                    self.running = None;
                    app.index_status = None;
                    match result {
                        Ok(count) => {
                            app.last_inference = Some(report);
                            app.inference_saved = true;
                            app.notice(format!(
                                "Updated {count} schema files with inferred types and index rules"
                            ));
                        }
                        Err(error) => app.error(format!("Schema write failed: {error}")),
                    }
                    if app.queued_search {
                        app.search();
                    }
                    break;
                }
                Ok(SearchUpdate::Finished {
                    mut repository,
                    total,
                    truncated,
                    elapsed_ms,
                }) => {
                    std::mem::swap(app.repo, &mut *repository);
                    self.running = None;
                    if current && kind == JobKind::Backlinks {
                        app.links.status = match total {
                            Ok(_) => BacklinkStatus::Complete,
                            Err(error) => {
                                app.links.incoming.clear();
                                BacklinkStatus::Error(error)
                            }
                        };
                    } else if current && kind == JobKind::Search {
                        app.finish_search(
                            total.map(|total| (total, truncated, elapsed_ms.unwrap_or(0.0))),
                        );
                    }
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.running = None;
                    if current && kind == JobKind::Backlinks {
                        app.links.incoming.clear();
                        app.links.status =
                            BacklinkStatus::Error("Backlink worker stopped unexpectedly".into());
                    } else if current && matches!(kind, JobKind::Inference | JobKind::SchemaWrite) {
                        app.index_status = None;
                        app.error("Inference worker stopped unexpectedly");
                    } else if current {
                        app.finish_search(Err("Search worker stopped unexpectedly".into()));
                    }
                    break;
                }
            }
        }
        if self.running.is_none() && app.index_status.is_none() && app.pending_schema_write {
            app.pending_schema_write = false;
            self.start_schema_write(app);
        } else if self.running.is_none()
            && app.index_status.is_none()
            && let Some(request) = app.pending_inference.take()
        {
            self.start_inference(app, request);
        } else if self.running.is_none()
            && (app.index_status.is_none() || app.reconciling)
            && let Some(request) = app.pending_search.take()
        {
            self.start(app, request);
        } else if self.running.is_none()
            && app.index_status.is_none()
            && let Some(target) = app.links.pending.take()
        {
            self.start_backlinks(app, target.collection, target.id);
        }
    }

    fn start_inference(&mut self, app: &mut App<'_>, request: InferRequest) {
        let placeholder = app.repo.fork_unindexed();
        let mut repository = std::mem::replace(app.repo, placeholder);
        let (sender, receiver) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        app.last_inference = None;
        app.inference_saved = false;
        app.index_status = Some(if request.auto_index {
            "Inferring fields and building advanced indexes…".into()
        } else {
            "Inferring fields…".into()
        });
        thread::spawn(move || {
            let result = repository
                .infer_and_auto_index(
                    request.collection.as_deref(),
                    InferOptions {
                        auto_index: request.auto_index,
                        write_schema: false,
                    },
                )
                .map_err(|error| error.to_string());
            let _ = sender.send(SearchUpdate::Inferred {
                repository: Box::new(repository),
                result,
            });
        });
        self.running = Some(Running {
            receiver,
            cancel,
            generation: 0,
            kind: JobKind::Inference,
        });
    }

    fn start_schema_write(&mut self, app: &mut App<'_>) {
        let Some(mut report) = app.last_inference.clone() else {
            return;
        };
        let placeholder = app.repo.fork_unindexed();
        let mut repository = std::mem::replace(app.repo, placeholder);
        let (sender, receiver) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        app.index_status = Some("Checking source and writing inferred schema rules…".into());
        thread::spawn(move || {
            let result = repository
                .write_inferred_schema(&mut report)
                .map_err(|error| error.to_string());
            let _ = sender.send(SearchUpdate::SchemaWritten {
                repository: Box::new(repository),
                report,
                result,
            });
        });
        self.running = Some(Running {
            receiver,
            cancel,
            generation: 0,
            kind: JobKind::SchemaWrite,
        });
    }

    fn start(&mut self, app: &mut App<'_>, request: Request) {
        // The UI keeps a lightweight handle while the worker owns the warm
        // repository. It is swapped back before another query or export.
        let placeholder = app.repo.fork_unindexed();
        let mut repository = std::mem::replace(app.repo, placeholder);
        // One complete page and its final event fit without blocking the
        // query on terminal redraws. Export uses a separate path.
        let (sender, receiver) = search_channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        thread::spawn(move || {
            let result = match request {
                Request::Filter(query) => {
                    let sorted = !query.sort.is_empty()
                        || objekte_core::split_sort_clause(&query.filter)
                            .is_ok_and(|(_, sorts)| !sorts.is_empty());
                    if sorted {
                        repository.query_json_stream_updates(&query, |page| {
                            send_page(
                                &sender,
                                &worker_cancel,
                                page.iter().cloned().map(Hit::Filter).collect(),
                            )
                        })
                    } else {
                        repository.query_json_stream(&query, |_, record| {
                            send_hit(&sender, &worker_cancel, Hit::Filter(record.clone()))
                        })
                    }
                    .map(|summary| (summary.total, summary.truncated, summary.elapsed_ms))
                }
                Request::Text(query) => {
                    let sorted = !query.sort.is_empty()
                        || objekte_core::split_sort_clause(&query.pattern)
                            .is_ok_and(|(_, sorts)| !sorts.is_empty());
                    if sorted {
                        repository.search_text_stream_updates(&query, |page| {
                            send_page(
                                &sender,
                                &worker_cancel,
                                page.iter().cloned().map(Hit::Text).collect(),
                            )
                        })
                    } else {
                        repository.search_text_stream(&query, |_, hit| {
                            send_hit(&sender, &worker_cancel, Hit::Text(hit.clone()))
                        })
                    }
                    .map(|summary| (summary.total, summary.truncated, summary.elapsed_ms))
                }
            }
            .map_err(|error| error.to_string());
            let elapsed_ms = result.as_ref().ok().map(|(_, _, elapsed_ms)| *elapsed_ms);
            let _ = sender.send(SearchUpdate::Finished {
                repository: Box::new(repository),
                total: result
                    .as_ref()
                    .map(|(total, _, _)| *total)
                    .map_err(Clone::clone),
                truncated: result.as_ref().is_ok_and(|(_, truncated, _)| *truncated),
                elapsed_ms,
            });
        });
        self.running = Some(Running {
            receiver,
            cancel,
            generation: app.generation,
            kind: JobKind::Search,
        });
    }

    fn start_backlinks(&mut self, app: &mut App<'_>, collection: String, id: String) {
        let requests = relations::backlink_requests(app.repo, &collection, &id);
        if requests.is_empty() {
            app.links.status = BacklinkStatus::Complete;
            return;
        }
        let placeholder = app.repo.fork_unindexed();
        let mut repository = std::mem::replace(app.repo, placeholder);
        let (sender, receiver) = mpsc::sync_channel(16);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        thread::spawn(move || {
            let result = requests.into_iter().try_for_each(|request| {
                repository.query_json_stream(&request, |_, record| {
                    send_backlink(&sender, &worker_cancel, record)
                })?;
                Ok::<_, Error>(())
            });
            let _ = sender.send(SearchUpdate::Finished {
                repository: Box::new(repository),
                total: result.map(|()| None).map_err(|error| error.to_string()),
                truncated: false,
                elapsed_ms: None,
            });
        });
        self.running = Some(Running {
            receiver,
            cancel,
            generation: app.links.generation,
            kind: JobKind::Backlinks,
        });
    }
}


fn search_channel() -> (mpsc::SyncSender<SearchUpdate>, Receiver<SearchUpdate>) {
    mpsc::sync_channel(PAGE_SIZE + 1)
}

fn send_hit(
    sender: &mpsc::SyncSender<SearchUpdate>,
    cancel: &AtomicBool,
    hit: Hit,
) -> objekte_core::Result<()> {
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Request("search cancelled".into()));
    }
    sender
        .send(SearchUpdate::Hit(Box::new(hit)))
        .map_err(|_| Error::Request("search UI disconnected".into()))
}

fn send_page(
    sender: &mpsc::SyncSender<SearchUpdate>,
    cancel: &AtomicBool,
    page: Vec<Hit>,
) -> objekte_core::Result<()> {
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Request("search cancelled".into()));
    }
    sender
        .send(SearchUpdate::Page(page))
        .map_err(|_| Error::Request("search UI disconnected".into()))
}

fn send_backlink(
    sender: &mpsc::SyncSender<SearchUpdate>,
    cancel: &AtomicBool,
    record: &Record,
) -> objekte_core::Result<()> {
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Request("backlink search cancelled".into()));
    }
    sender
        .send(SearchUpdate::Backlink(Box::new(record.clone())))
        .map_err(|_| Error::Request("backlink UI disconnected".into()))
}

#[cfg(test)]
#[path = "search.test.rs"]
mod tests;
