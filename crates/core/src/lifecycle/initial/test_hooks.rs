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

/// Check that conversion and borrowed text admission agree with owned strings.
pub fn assert_equivalent_logical_bigrams(
    schema: &crate::schema::Schema,
    record: &mut crate::source::ExtractedRecord,
    expected: &[&str],
) {
    use crate::index::bigram::{BigramAlphabet, BigramBuilder};
    use crate::model::RecordSlot;
    use crate::source::text::visit_logical_strings;

    crate::construction::file_preparation::finish_build_record(schema, record).unwrap();
    let mut strings = Vec::new();
    visit_logical_strings(&record.value, &mut |text| strings.push(text.to_owned()));
    assert_eq!(strings, expected);
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let mut borrowed = BigramBuilder::new(alphabet);
        borrowed
            .add_record_with(RecordSlot(0), |emit| {
                visit_logical_strings(&record.value, emit)
            })
            .unwrap();
        let borrowed = borrowed.finish().unwrap();
        let mut owned = BigramBuilder::new(alphabet);
        owned.add_record(RecordSlot(0), strings.iter()).unwrap();
        let owned = owned.finish().unwrap();
        assert_eq!(borrowed.normal, owned.normal);
        assert_eq!(borrowed.skip, owned.skip);
    }
}

/// A compiled text predicate for adapter scan tests.
pub struct ScanTextBinding(crate::query::binding::fts::BoundFts);

impl<'a> From<&'a ScanTextBinding> for crate::source::verification::ScalarText<'a> {
    fn from(binding: &'a ScanTextBinding) -> Self {
        (&binding.0).into()
    }
}

/// A compiled single-term predicate for adapter scan tests.
pub struct ScanFilterBinding(crate::query::binding::filter::BoundTerm);

impl<'a> From<&'a ScanFilterBinding> for crate::source::verification::ScalarFilter<'a> {
    fn from(binding: &'a ScanFilterBinding) -> Self {
        (&binding.0).into()
    }
}

pub fn bind_scan_text(
    fts: &crate::query::FtsQuery,
    schema: &crate::schema::Schema,
) -> Result<ScanTextBinding, String> {
    crate::query::binding::fts::bind_fts(fts, schema).map(ScanTextBinding)
}

pub fn bind_scan_filter(
    filter: &crate::query::FilterExpr,
    schema: &crate::schema::Schema,
) -> Result<ScanFilterBinding, String> {
    use crate::query::binding::filter::{BoundExpr, bind_filter};
    match bind_filter(filter, schema)? {
        BoundExpr::Term(term) => Ok(ScanFilterBinding(term)),
        _ => Err("scan fixture requires a single term".into()),
    }
}

#[cfg(feature = "test-scan-instrumentation")]
pub fn fold_candidate_transforms() -> usize {
    crate::query::semantics::folding::fold_candidate_transforms()
}

#[cfg(feature = "test-scan-instrumentation")]
pub fn fold_scratch_stats() -> (usize, usize) {
    crate::query::semantics::folding::fold_scratch_stats()
}
