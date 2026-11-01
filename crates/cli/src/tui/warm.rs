//! Background construction of indexes after the repository model is parsed.

use objekte_core::{CollectionInventory, Repository};
use std::{
    sync::mpsc::{self, Receiver},
    thread,
};

/// Progress from the worker, followed by one final repository or error.
pub(super) enum WarmUpdate {
    Progress {
        completed: usize,
        total: usize,
        mode: WarmMode,
        restored: u64,
    },
    Ready(Box<Repository>, WarmMode),
    Inventory(CollectionInventory),
    Checked,
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum WarmMode {
    Warm,
    Save,
    Reindex,
}

pub(super) fn startup_status(repo: &Repository) -> String {
    let total = repo.model().collections.len();
    let has_cache = repo.model().collections.keys().any(|name| {
        repo.bigram_cache_path(name)
            .is_ok_and(|path| path.is_file())
    });
    let action = if has_cache { "Restoring" } else { "Building" };
    format!("{action} indexes · 0/{total} collections ready")
}

pub(super) fn start(repo: &Repository, mode: WarmMode) -> Receiver<WarmUpdate> {
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
        if mode == WarmMode::Warm {
            let mut restored_names = Vec::new();
            for (index, name) in names.iter().enumerate() {
                match building.restore_cached_index_unchecked(name) {
                    Ok(true) => restored_names.push(name.clone()),
                    Ok(false) => {
                        if let Err(error) = building.warm_bigram_index(name) {
                            let _ = sender.send(WarmUpdate::Failed(error.to_string()));
                            return;
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(WarmUpdate::Failed(error.to_string()));
                        return;
                    }
                }
                if sender
                    .send(WarmUpdate::Progress {
                        completed: index + 1,
                        total,
                        mode,
                        restored: building.metrics().bigram_snapshot_loads,
                    })
                    .is_err()
                {
                    return;
                }
            }
            let checker = building.fork_unindexed();
            if sender
                .send(WarmUpdate::Ready(Box::new(building), mode))
                .is_err()
            {
                return;
            }
            for name in restored_names {
                match checker.collect_collection_inventory(&name) {
                    Ok(inventory) => {
                        if sender.send(WarmUpdate::Inventory(inventory)).is_err() {
                            return;
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(WarmUpdate::Failed(error.to_string()));
                        return;
                    }
                }
            }
            let _ = sender.send(WarmUpdate::Checked);
            return;
        }
        for (index, name) in names.iter().enumerate() {
            let result = match mode {
                WarmMode::Warm => building.warm_bigram_index(name).map(|_| ()),
                WarmMode::Save => building.save_bigram_cache(name).map(|_| ()),
                WarmMode::Reindex => building.reindex_bigram_cache(name).map(|_| ()),
            };
            if let Err(error) = result {
                let _ = sender.send(WarmUpdate::Failed(error.to_string()));
                return;
            }
            if sender
                .send(WarmUpdate::Progress {
                    completed: index + 1,
                    total,
                    mode,
                    restored: building.metrics().secondary_snapshot_loads,
                })
                .is_err()
            {
                return;
            }
        }
        let _ = sender.send(WarmUpdate::Ready(Box::new(building), mode));
    });
    receiver
}
