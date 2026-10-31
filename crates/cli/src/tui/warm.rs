//! Background construction of indexes after the repository model is parsed.

use objekte_core::Repository;
use std::{
    sync::mpsc::{self, Receiver},
    thread,
};


/// Progress from the worker, followed by one final repository or error.
pub(super) enum WarmUpdate {
    Progress { completed: usize, total: usize },
    Ready(Box<Repository>),
    Failed(String),
}


pub(super) fn start(repo: &Repository) -> Receiver<WarmUpdate> {
    let mut building = repo.fork_unindexed();
    let names = building
        .model()
        .collections
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    let total = names.len();
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for (index, name) in names.iter().enumerate() {
            if let Err(error) = building.warm_bigram_index(name) {
                let _ = sender.send(WarmUpdate::Failed(error.to_string()));
                return;
            }
            if sender
                .send(WarmUpdate::Progress {
                    completed: index + 1,
                    total,
                })
                .is_err()
            {
                return;
            }
        }
        let _ = sender.send(WarmUpdate::Ready(Box::new(building)));
    });
    receiver
}
