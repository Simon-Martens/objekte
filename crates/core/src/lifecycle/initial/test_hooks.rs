//! Test-only observation of the real backend's pull-framing boundary.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

type Hook = Arc<dyn Fn(&Path) + Send + Sync>;

static HOOK: Mutex<Option<(PathBuf, Hook)>> = Mutex::new(None);

pub struct PullFrameHook;

impl Drop for PullFrameHook {
    fn drop(&mut self) {
        *HOOK.lock().unwrap() = None;
    }
}

pub fn install_pull_frame_hook(
    root: PathBuf,
    callback: impl Fn(&Path) + Send + Sync + 'static,
) -> PullFrameHook {
    let mut hook = HOOK.lock().unwrap();
    assert!(hook.is_none(), "pull framing hook already installed");
    *hook = Some((root, Arc::new(callback)));
    PullFrameHook
}

pub(crate) fn before_pull_frame(path: &Path) {
    let callback = HOOK
        .lock()
        .unwrap()
        .as_ref()
        .filter(|(root, _)| path.starts_with(root))
        .map(|(_, callback)| Arc::clone(callback));
    if let Some(callback) = callback {
        callback(path);
    }
}

use std::sync::atomic::{AtomicUsize, Ordering};

static FILE_ACTIVITY: Mutex<Option<(PathBuf, Arc<Activity>)>> = Mutex::new(None);

#[derive(Default)]
struct Activity {
    active: AtomicUsize,
    peak: AtomicUsize,
}

pub struct FileActivityProbe(Arc<Activity>);

impl FileActivityProbe {
    pub fn peak(&self) -> usize {
        self.0.peak.load(Ordering::Relaxed)
    }
}

impl Drop for FileActivityProbe {
    fn drop(&mut self) {
        *FILE_ACTIVITY.lock().unwrap() = None;
    }
}

pub fn track_file_activity(root: PathBuf) -> FileActivityProbe {
    let mut current = FILE_ACTIVITY.lock().unwrap();
    assert!(current.is_none(), "file activity probe already installed");
    let activity = Arc::new(Activity::default());
    *current = Some((root, Arc::clone(&activity)));
    FileActivityProbe(activity)
}

pub(crate) struct ActiveFile(Arc<Activity>);

impl Drop for ActiveFile {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::Relaxed);
    }
}

pub(crate) fn enter_file(path: &Path) -> Option<ActiveFile> {
    let current = FILE_ACTIVITY.lock().unwrap();
    let (_, activity) = current
        .as_ref()
        .filter(|(root, _)| path.starts_with(root))?;
    let active = activity.active.fetch_add(1, Ordering::Relaxed) + 1;
    activity.peak.fetch_max(active, Ordering::Relaxed);
    Some(ActiveFile(Arc::clone(activity)))
}

pub use crate::construction::file_shard_diagnostics::measure_direct_shard_build;
