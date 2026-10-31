//! Background query execution and ordered hit events for the terminal UI.

use super::{
    navigation::BacklinkStatus,
    relations,
    state::{App, DetailEdge, Focus, Hit, Request},
};
use objekte_core::{Error, Record, Repository};
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
    Backlink(Box<Record>),
    Finished {
        repository: Box<Repository>,
        total: Result<Option<usize>, String>,
    },
}


/// Which generation a repository-owning job belongs to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum JobKind {
    Search,
    Backlinks,
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
                JobKind::Search => app.search_generation,
                JobKind::Backlinks => app.links.generation,
            }
    }
}


/// Drives one repository-owning worker at a time from the TUI event loop.
#[derive(Default)]
pub(super) struct SearchWorker {
    running: Option<Running>,
}


impl SearchWorker {
    pub(super) fn drive(&mut self, app: &mut App<'_>) {
        if let Some(running) = &self.running
            && !running.current(app)
        {
            running.cancel.store(true, Ordering::Relaxed);
        }
        while let Some(running) = &self.running {
            match running.receiver.try_recv() {
                Ok(SearchUpdate::Hit(hit)) => {
                    if running.kind == JobKind::Search && running.current(app) && app.searching {
                        app.hits.push(*hit);
                    }
                }
                Ok(SearchUpdate::Backlink(record)) => {
                    if running.kind == JobKind::Backlinks && running.current(app) {
                        app.add_backlink_record(&record);
                    }
                }
                Ok(SearchUpdate::Finished {
                    mut repository,
                    total,
                }) => {
                    let current = running.current(app);
                    let kind = running.kind;
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
                    } else if current {
                        app.searching = false;
                        match total {
                            Ok(total) => {
                                app.total = total;
                                if let Some(edge) = app.pending_detail.take() {
                                    if app.hits.is_empty() {
                                        app.focus = Focus::Results;
                                        app.message = "No results on this page.".into();
                                    } else {
                                        app.selected = if edge == DetailEdge::Last {
                                            app.hits.len() - 1
                                        } else {
                                            0
                                        };
                                        app.focus = Focus::Detail;
                                        app.detail_scroll = 0;
                                        app.detail_position();
                                        app.prepare_detail_links();
                                    }
                                } else if app.focus == Focus::Detail && !app.hits.is_empty() {
                                    app.detail_position();
                                } else {
                                    app.message.clear();
                                }
                            }
                            Err(error) => {
                                // Streamed rows are provisional until the
                                // complete query succeeds.
                                app.hits.clear();
                                app.total = None;
                                app.last_request = None;
                                app.query_fields.clear();
                                app.pending_detail = None;
                                app.links.reset();
                                app.focus = Focus::Query;
                                app.message = error;
                            }
                        }
                    }
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    let current = running.current(app);
                    let kind = running.kind;
                    self.running = None;
                    if current && kind == JobKind::Backlinks {
                        app.links.incoming.clear();
                        app.links.status =
                            BacklinkStatus::Error("Backlink worker stopped unexpectedly.".into());
                    } else if current {
                        app.searching = false;
                        app.hits.clear();
                        app.total = None;
                        app.last_request = None;
                        app.query_fields.clear();
                        app.pending_detail = None;
                        app.links.reset();
                        app.message = "Search worker stopped unexpectedly.".into();
                        app.focus = Focus::Query;
                    }
                    break;
                }
            }
        }
        if self.running.is_none()
            && app.index_status.is_none()
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

    fn start(&mut self, app: &mut App<'_>, request: Request) {
        // The UI keeps a lightweight handle while the worker owns the warm
        // repository. It is swapped back before another query or export.
        let placeholder = app.repo.fork_unindexed();
        let mut repository = std::mem::replace(app.repo, placeholder);
        // Bound queued rows so large JSON records cannot accumulate behind a
        // busy terminal renderer.
        let (sender, receiver) = mpsc::sync_channel(16);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        thread::spawn(move || {
            let total = match request {
                Request::Filter(query) => repository
                    .query_json_stream(&query, |_, record| {
                        send_hit(&sender, &worker_cancel, Hit::Filter(record.clone()))
                    })
                    .map(|summary| summary.total),
                Request::Text(query) => repository
                    .search_text_stream(&query, |_, hit| {
                        send_hit(&sender, &worker_cancel, Hit::Text(hit.clone()))
                    })
                    .map(|summary| summary.total),
            }
            .map_err(|error| error.to_string());
            let _ = sender.send(SearchUpdate::Finished {
                repository: Box::new(repository),
                total,
            });
        });
        self.running = Some(Running {
            receiver,
            cancel,
            generation: app.search_generation,
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
