use std::io::{BufWriter, Write};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

use objekte_search_core::index::segment::SegmentPolicy;
use objekte_search_core::lifecycle::initial::{
    InitialBuildPhase, InitialBuildTask, test_hooks::install_pull_frame_hook,
};
use objekte_search_core::lifecycle::{IndexRevision, IndexView};
use objekte_search_core::repository::configuration::IdentityPolicy;

use super::*;

const RECORDS: usize = 9_000;
const TIMEOUT: Duration = Duration::from_secs(5);

struct LargeFixture {
    root: tempfile::TempDir,
    files: usize,
    total_bytes: u64,
    locators: Vec<(String, PathBuf, ByteRange)>,
}

impl LargeFixture {
    fn new(files: usize) -> Self {
        let mut fixture = Self {
            root: tempfile::tempdir().unwrap(),
            files,
            total_bytes: 0,
            locators: Vec::new(),
        };
        let padding = "x".repeat(1024);
        for file in 0..files {
            let path = PathBuf::from(format!("{file}.jsonl"));
            let mut output =
                BufWriter::new(std::fs::File::create(fixture.root.path().join(&path)).unwrap());
            let mut offset = 0;
            for number in 0..RECORDS {
                let id = record_id(file, number);
                let line = format!(r#"{{"id":"{id}","title":"{padding}"}}"#);
                let end = offset + line.len() as u64;
                if [0, RECORDS / 2, RECORDS - 1].contains(&number) {
                    fixture
                        .locators
                        .push((id, path.clone(), ByteRange { start: offset, end }));
                }
                writeln!(output, "{line}").unwrap();
                offset = end + 1;
            }
            output.flush().unwrap();
            assert!(offset > 8 * 1024 * 1024);
            fixture.total_bytes += offset;
        }
        fixture
    }

    fn start(&self, workers: usize, names: &[&str]) -> InitialBuildTask {
        let mut sources = SourceRegistry::default();
        objekte_loader_jsonl::register(&mut sources).unwrap();
        let collections = names
            .iter()
            .map(|name| {
                let mut collection =
                    CollectionDefinition::folder(*name, "", objekte_loader_jsonl::JSON_LINES);
                collection.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
                collection
            })
            .collect();
        let options = RepositoryOptions {
            build_workers: NonZeroUsize::new(workers),
            segment_policy: SegmentPolicy {
                target_records: NonZeroUsize::new(1024).unwrap(),
                max_files: NonZeroUsize::new(1).unwrap(),
            },
            ..Default::default()
        };
        Index::start_build(self.root.path(), collections, options, sources).unwrap()
    }

    fn verify(&self, index: Index, names: &[&str]) {
        let view = index.view().unwrap();
        let status = index.status().unwrap();
        assert_eq!(
            status.live_records,
            (self.files * RECORDS * names.len()) as u64
        );
        assert_eq!(status.build_source_reads, self.files as u64);
        assert_eq!(status.build_source_bytes_read, self.total_bytes);
        assert_eq!(
            view.revision().0 - IndexRevision::INITIAL.0 - 1,
            self.files as u64
        );
        let catalog = view.file_catalog().unwrap();
        for (id, path, range) in &self.locators {
            let file = catalog.file_by_path(path).unwrap().id;
            for name in names {
                let hit = view
                    .get_record(name, id, ReturnMode::Locator)
                    .unwrap()
                    .unwrap();
                assert_eq!(hit.source_range(), Some(*range));
                assert_eq!(hit.file.unwrap().file_id, file);
            }
            assert_eq!(
                catalog.records_for_file(file).unwrap().len(),
                RECORDS * names.len()
            );
        }
    }
}

fn record_id(file: usize, number: usize) -> String {
    format!("r{file}-{number:05}")
}

fn check_independent_publication(
    view: &IndexView,
    held: usize,
    names: &[&str],
) -> Result<bool, String> {
    let peer_id = record_id(1 - held, 0);
    let mut request = query(names, ReturnMode::Locator);
    request.filter = Some(parse_filter(&format!("@record.id = '{peer_id}'"))?);
    let result = view.search_available(&request)?;
    let found = result
        .result
        .hits
        .iter()
        .filter(|hit| hit.record_id == peer_id)
        .count();
    if found == 0 {
        return Ok(false);
    }
    if found != names.len() {
        return Err(format!(
            "peer published {found} of {} bindings",
            names.len()
        ));
    }
    let catalog = view.file_catalog().ok_or("missing catalog")?;
    let row = catalog
        .file_by_path(Path::new(&format!("{held}.jsonl")))
        .ok_or("held file missing from catalog")?;
    let count = catalog
        .records_for_file(row.id)
        .map_err(|error| error.to_string())?
        .len();
    if count != 0 {
        return Err(format!(
            "held file published {count} records before release"
        ));
    }
    Ok(true)
}

fn verify_gated_build(fixture: &LargeFixture, names: &[&str], cancel: bool) {
    let (entered, first) = mpsc::sync_channel(1);
    let (release, gate) = mpsc::channel::<()>();
    let gate = Mutex::new(Some(gate));
    let hook = install_pull_frame_hook(fixture.root.path().canonicalize().unwrap(), move |path| {
        let first = gate.lock().unwrap().take();
        if let Some(gate) = first {
            let file = path
                .file_stem()
                .unwrap()
                .to_str()
                .unwrap()
                .parse::<usize>()
                .unwrap();
            if entered.send(file).is_ok() {
                let _ = gate.recv();
            }
        }
    });

    let task = fixture.start(4, names);
    let deadline = Instant::now() + TIMEOUT;
    let first = first.recv_timeout(TIMEOUT);
    let (observed, finished) = std::thread::scope(|scope| {
        let (observations, result) = mpsc::sync_channel(1);
        let ready = first.ok().map(|held| {
            let task = &task;
            scope.spawn(move || {
                let observe = || -> Result<usize, String> {
                    let mut snapshot = task.snapshot().map_err(|error| error.to_string())?;
                    loop {
                        if let Some(view) = &snapshot.view
                            && check_independent_publication(view, held, names)?
                        {
                            return Ok(held);
                        }
                        if matches!(
                            snapshot.status.phase,
                            InitialBuildPhase::Complete
                                | InitialBuildPhase::Cancelled
                                | InitialBuildPhase::Failed
                        ) {
                            return Err(format!(
                                "build reached {:?} before peer visibility",
                                snapshot.status.phase
                            ));
                        }
                        snapshot = task
                            .wait_next(snapshot.cursor)
                            .map_err(|error| error.to_string())?;
                    }
                };
                let _ = observations.send(observe());
            })
        });
        let observed = if ready.is_some() {
            result
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .map_err(|error| {
                    format!("peer never published while first framer was held: {error}")
                })
                .and_then(|result| result)
        } else {
            Err("first framer did not reach the gate".into())
        };
        if cancel && observed.is_ok() {
            task.cancel();
        }
        drop(release);
        let finished = task.wait_complete();
        (observed, finished)
    });
    drop(hook);
    let held = observed.unwrap();
    if cancel {
        assert!(matches!(
            finished,
            Err(objekte_search_core::lifecycle::IndexError::Cancelled)
        ));
        let view = task.snapshot().unwrap().view.unwrap();
        assert!(check_independent_publication(&view, held, names).unwrap());
    } else {
        fixture.verify(finished.unwrap(), names);
    }
}

#[test]
fn large_jsonl_peer_publishes_while_first_framer_is_held() {
    let fixture = LargeFixture::new(2);
    let names = ["lines"];
    verify_gated_build(&fixture, &names, false);
    fixture.verify(fixture.start(1, &names).wait_complete().unwrap(), &names);

    let names = ["lines", "shared"];
    verify_gated_build(&fixture, &names, false);
    fixture.verify(fixture.start(1, &names).wait_complete().unwrap(), &names);
    verify_gated_build(&fixture, &names, true);
}

#[test]
fn several_large_jsonl_files_preserve_one_pass_locators_and_publication() {
    let fixture = LargeFixture::new(5);
    let names = ["lines"];
    fixture.verify(fixture.start(4, &names).wait_complete().unwrap(), &names);
}
