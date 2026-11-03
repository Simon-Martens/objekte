//! Read-only candidate I/O experiment. No query executor uses these backends.

#[cfg(feature = "candidate-allocations")]
mod allocation {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};

    struct Counter;
    static ALLOCS: AtomicUsize = AtomicUsize::new(0);
    static BYTES: AtomicUsize = AtomicUsize::new(0);
    static LIVE: AtomicIsize = AtomicIsize::new(0);
    static PEAK: AtomicIsize = AtomicIsize::new(0);
    #[global_allocator]
    static ALLOCATOR: Counter = Counter;

    unsafe impl GlobalAlloc for Counter {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let ptr = unsafe { System.alloc(layout) };
            if !ptr.is_null() {
                record(layout.size(), layout.size() as isize);
            }
            ptr
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            let ptr = unsafe { System.alloc_zeroed(layout) };
            if !ptr.is_null() {
                record(layout.size(), layout.size() as isize);
            }
            ptr
        }
        unsafe fn realloc(&self, ptr: *mut u8, old: Layout, new_size: usize) -> *mut u8 {
            let result = unsafe { System.realloc(ptr, old, new_size) };
            if !result.is_null() {
                record(new_size, new_size as isize - old.size() as isize);
            }
            result
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
            unsafe {
                System.dealloc(ptr, layout);
            }
        }
    }
    fn record(bytes: usize, live_delta: isize) {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(bytes, Ordering::Relaxed);
        let live = LIVE.fetch_add(live_delta, Ordering::Relaxed) + live_delta;
        PEAK.fetch_max(live, Ordering::Relaxed);
    }
    pub fn start() -> (usize, usize, isize) {
        let live = LIVE.load(Ordering::Relaxed);
        PEAK.store(live, Ordering::Relaxed);
        (
            ALLOCS.load(Ordering::Relaxed),
            BYTES.load(Ordering::Relaxed),
            live,
        )
    }
    pub fn finish(before: (usize, usize, isize)) -> (usize, usize, isize) {
        (
            ALLOCS.load(Ordering::Relaxed) - before.0,
            BYTES.load(Ordering::Relaxed) - before.1,
            PEAK.load(Ordering::Relaxed) - before.2,
        )
    }
}

#[cfg(not(feature = "candidate-allocations"))]
mod allocation {
    pub fn start() -> (usize, usize, isize) {
        (0, 0, 0)
    }
    pub fn finish(_: (usize, usize, isize)) -> (usize, usize, isize) {
        (0, 0, 0)
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use compio::{fs::File as CompioFile, io::AsyncReadAtExt, runtime::Runtime};
    use futures_util::{StreamExt, stream};
    use io_uring::{IoUring, opcode, types};
    use objekte_core::{BigramEncoding, Repository, file_picker::FilePicker};
    use rayon::prelude::*;
    use serde_json::json;
    use std::{
        cell::RefCell,
        env,
        fs::{self, File, Metadata},
        hash::{Hash, Hasher},
        io::{self, Read},
        os::{
            fd::{AsRawFd, FromRawFd, IntoRawFd},
            unix::{ffi::OsStrExt, fs::MetadataExt},
        },
        path::{Path, PathBuf},
        rc::Rc,
        sync::{Arc, mpsc},
        thread,
        time::Instant,
    };

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct Stamp {
        size: u64,
        dev: u64,
        ino: u64,
        mtime: (i64, i64),
        ctime: (i64, i64),
    }

    fn stamp(meta: &Metadata) -> io::Result<Stamp> {
        if !meta.is_file() || meta.len() > i64::MAX as u64 {
            return Err(io::Error::other("source is not a supported regular file"));
        }
        Ok(Stamp {
            size: meta.len(),
            dev: meta.dev(),
            ino: meta.ino(),
            mtime: (meta.mtime(), meta.mtime_nsec()),
            ctime: (meta.ctime(), meta.ctime_nsec()),
        })
    }

    type DirectoryStamp = (u64, u64, i64, i64, i64, i64);
    fn dir_stamp(meta: &Metadata) -> io::Result<DirectoryStamp> {
        if !meta.is_dir() {
            return Err(io::Error::other("source root is not a directory"));
        }
        Ok((
            meta.dev(),
            meta.ino(),
            meta.mtime(),
            meta.mtime_nsec(),
            meta.ctime(),
            meta.ctime_nsec(),
        ))
    }

    fn open_root(path: &Path) -> io::Result<File> {
        if fs::canonicalize(path)? != path {
            return Err(io::Error::other("source root changed"));
        }
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(io::Error::other)?;
        // SAFETY: the root name is NUL-terminated and the successful descriptor is owned here.
        let fd = unsafe {
            libc::openat(
                libc::AT_FDCWD,
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let file = unsafe { File::from_raw_fd(fd) };
        if dir_stamp(&file.metadata()?)? != dir_stamp(&fs::symlink_metadata(path)?)? {
            return Err(io::Error::other("source root changed while opening"));
        }
        Ok(file)
    }

    fn descriptor_stamp(fd: i32) -> io::Result<Stamp> {
        let mut value = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: fstat initializes `value` on success and the caller owns `fd`.
        if unsafe { libc::fstat(fd, value.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let value = unsafe { value.assume_init() };
        if value.st_mode & libc::S_IFMT != libc::S_IFREG || value.st_size < 0 {
            return Err(io::Error::other("source is not a supported regular file"));
        }
        Ok(Stamp {
            size: value.st_size as u64,
            dev: value.st_dev,
            ino: value.st_ino,
            mtime: (value.st_mtime, value.st_mtime_nsec),
            ctime: (value.st_ctime, value.st_ctime_nsec),
        })
    }

    struct Request {
        id: String,
        expected: Stamp,
    }
    struct Context {
        root: PathBuf,
        root_fd: File,
        root_stamp: DirectoryStamp,
        requests: Vec<Request>,
    }

    impl Context {
        fn fork(&self) -> io::Result<Self> {
            Ok(Self {
                root: self.root.clone(),
                root_fd: self.root_fd.try_clone()?,
                root_stamp: self.root_stamp,
                requests: self
                    .requests
                    .iter()
                    .map(|r| Request {
                        id: r.id.clone(),
                        expected: r.expected,
                    })
                    .collect(),
            })
        }

        fn open(&self, index: usize) -> io::Result<File> {
            let id = &self.requests[index].id;
            let mut parent: Option<File> = None;
            let mut parts = id.split('/').peekable();
            while let Some(part) = parts.next() {
                if part.is_empty() || part == "." || part == ".." || part.contains('\0') {
                    return Err(io::Error::other("invalid candidate ID"));
                }
                let name = if parts.peek().is_some() {
                    part.to_owned()
                } else {
                    format!("{part}.json")
                };
                let name = std::ffi::CString::new(name).map_err(io::Error::other)?;
                let flags = if parts.peek().is_some() {
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC
                } else {
                    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK
                };
                // SAFETY: the pinned parent descriptor and NUL-terminated name are valid.
                let fd = unsafe {
                    libc::openat(
                        parent.as_ref().unwrap_or(&self.root_fd).as_raw_fd(),
                        name.as_ptr(),
                        flags,
                    )
                };
                if fd < 0 {
                    return Err(io::Error::last_os_error());
                }
                parent = Some(unsafe { File::from_raw_fd(fd) });
            }
            parent.ok_or_else(|| io::Error::other("empty candidate ID"))
        }

        fn check_root(&self) -> io::Result<()> {
            if dir_stamp(&fs::symlink_metadata(&self.root)?)? != self.root_stamp {
                return Err(io::Error::other("source directory changed"));
            }
            Ok(())
        }
    }

    fn fingerprint(bytes: &[u8], verify: bool) -> u64 {
        if !verify {
            return 0;
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut h);
        h.finish()
    }

    fn sync_one(
        ctx: &Context,
        index: usize,
        bytes: &mut Vec<u8>,
        verify: bool,
    ) -> io::Result<(u64, u64)> {
        let mut file = ctx.open(index)?;
        let before = stamp(&file.metadata()?)?;
        if before != ctx.requests[index].expected {
            return Err(io::Error::other("candidate changed before open"));
        }
        bytes.clear();
        file.read_to_end(bytes)?;
        let after = stamp(&file.metadata()?)?;
        if before != after || bytes.len() as u64 != after.size {
            return Err(io::Error::other("candidate changed during read"));
        }
        Ok((after.size, fingerprint(bytes, verify)))
    }

    type ReadResults = Vec<(u64, u64)>;
    fn rayon_reads(
        ctx: &Context,
        indices: &[usize],
        pool: &rayon::ThreadPool,
        verify: bool,
    ) -> io::Result<ReadResults> {
        pool.install(|| {
            indices
                .par_iter()
                .map_init(Vec::new, |bytes, &index| {
                    sync_one(ctx, index, bytes, verify)
                })
                .collect()
        })
    }

    struct SyncJob {
        indices: Vec<usize>,
        verify: bool,
        reply: mpsc::Sender<io::Result<ReadResults>>,
    }
    struct SyncWorker {
        jobs: mpsc::Sender<SyncJob>,
        handle: Option<thread::JoinHandle<()>>,
        init_nanos: u128,
    }
    impl SyncWorker {
        fn new(ctx: Arc<Context>, workers: usize) -> io::Result<Self> {
            let (jobs, rx) = mpsc::channel::<SyncJob>();
            let (ready, ready_rx) = mpsc::channel();
            let started = Instant::now();
            let handle = thread::spawn(move || {
                let pool = match rayon::ThreadPoolBuilder::new().num_threads(workers).build() {
                    Ok(pool) => {
                        let _ = ready.send(Ok(()));
                        pool
                    }
                    Err(error) => {
                        let _ = ready.send(Err(io::Error::other(error)));
                        return;
                    }
                };
                for job in rx {
                    let result = rayon_reads(&ctx, &job.indices, &pool, job.verify);
                    let _ = job.reply.send(result);
                }
            });
            ready_rx.recv().map_err(io::Error::other)??;
            Ok(Self {
                jobs,
                handle: Some(handle),
                init_nanos: started.elapsed().as_nanos(),
            })
        }
        fn read(&self, indices: &[usize], verify: bool) -> io::Result<ReadResults> {
            let (reply, rx) = mpsc::channel();
            self.jobs
                .send(SyncJob {
                    indices: indices.to_vec(),
                    verify,
                    reply,
                })
                .map_err(io::Error::other)?;
            rx.recv().map_err(io::Error::other)?
        }
    }
    impl Drop for SyncWorker {
        fn drop(&mut self) {
            let (unused, _) = mpsc::channel();
            drop(std::mem::replace(&mut self.jobs, unused));
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    async fn compio_one(
        ctx: &Context,
        index: usize,
        verify: bool,
        mut buffer: Vec<u8>,
    ) -> io::Result<(usize, (u64, u64), Vec<u8>)> {
        let opened = ctx.open(index)?;
        let before = stamp(&opened.metadata()?)?;
        if before != ctx.requests[index].expected {
            return Err(io::Error::other("candidate changed before open"));
        }
        // SAFETY: into_raw_fd transfers sole ownership; CompioFile closes it once.
        let file = unsafe { CompioFile::from_raw_fd(opened.into_raw_fd()) };
        buffer.clear();
        if buffer.capacity() < before.size as usize {
            buffer.reserve(before.size as usize);
        }
        let (read, bytes) = file.read_to_end_at(buffer, 0).await.into_parts();
        let read = read?;
        let after = descriptor_stamp(file.as_raw_fd())?;
        if before != after || read as u64 != after.size || bytes.len() as u64 != after.size {
            return Err(io::Error::other("candidate changed during read"));
        }
        Ok((index, (after.size, fingerprint(&bytes, verify)), bytes))
    }

    struct CompioJob {
        indices: Vec<usize>,
        depth: usize,
        verify: bool,
        reply: mpsc::Sender<io::Result<ReadResults>>,
    }
    struct CompioWorker {
        jobs: mpsc::Sender<CompioJob>,
        handle: Option<thread::JoinHandle<()>>,
        init_nanos: u128,
    }
    impl CompioWorker {
        fn new(ctx: Arc<Context>) -> io::Result<Self> {
            let (jobs, rx) = mpsc::channel::<CompioJob>();
            let (ready, ready_rx) = mpsc::channel();
            let started = Instant::now();
            let handle = thread::spawn(move || {
                let runtime = match Runtime::new() {
                    Ok(runtime) => {
                        let _ = ready.send(Ok(()));
                        runtime
                    }
                    Err(error) => {
                        let _ = ready.send(Err(io::Error::other(error)));
                        return;
                    }
                };
                let buffers = Rc::new(RefCell::new(Vec::<Vec<u8>>::new()));
                for job in rx {
                    let result = runtime.block_on(async {
                        let mut results = vec![(0, 0); job.indices.len()];
                        let context = &ctx;
                        let verify = job.verify;
                        let pool = &buffers;
                        let mut reads = stream::iter(job.indices.iter().copied().enumerate().map(
                            |(position, index)| {
                                let pool = Rc::clone(pool);
                                async move {
                                    let buffer = pool.borrow_mut().pop().unwrap_or_default();
                                    compio_one(context, index, verify, buffer)
                                        .await
                                        .map(|(_, value, bytes)| (position, value, bytes))
                                }
                            },
                        ))
                        .buffer_unordered(job.depth);
                        let mut first_error = None;
                        while let Some(read) = reads.next().await {
                            match read {
                                Ok((position, value, mut bytes)) => {
                                    results[position] = value;
                                    bytes.clear();
                                    buffers.borrow_mut().push(bytes);
                                }
                                Err(error) => {
                                    if first_error.is_none() {
                                        first_error = Some(error);
                                    }
                                }
                            }
                        }
                        first_error.map_or(Ok(results), Err)
                    });
                    let _ = job.reply.send(result);
                }
            });
            ready_rx.recv().map_err(io::Error::other)??;
            Ok(Self {
                jobs,
                handle: Some(handle),
                init_nanos: started.elapsed().as_nanos(),
            })
        }
        fn read(&self, indices: &[usize], depth: usize, verify: bool) -> io::Result<ReadResults> {
            let (reply, rx) = mpsc::channel();
            self.jobs
                .send(CompioJob {
                    indices: indices.to_vec(),
                    depth,
                    verify,
                    reply,
                })
                .map_err(io::Error::other)?;
            rx.recv().map_err(io::Error::other)?
        }
    }
    impl Drop for CompioWorker {
        fn drop(&mut self) {
            // Closing the request channel lets the dedicated runtime exit cleanly.
            let (unused, _) = mpsc::channel();
            let old = std::mem::replace(&mut self.jobs, unused);
            drop(old);
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    #[derive(Default)]
    struct Pending {
        file: Option<File>,
        before: Option<Stamp>,
        bytes: Vec<u8>,
        offset: usize,
        request: usize,
    }
    fn push_read(ring: &mut IoUring, slot: usize, pending: &mut Pending) -> io::Result<()> {
        let remaining = pending.bytes.len() - pending.offset;
        let len = u32::try_from(remaining).map_err(io::Error::other)?;
        // SAFETY: this allocation is neither resized nor freed before its CQE.
        let ptr = unsafe { pending.bytes.as_mut_ptr().add(pending.offset) };
        let entry = opcode::Read::new(
            types::Fd(pending.file.as_ref().unwrap().as_raw_fd()),
            ptr,
            len,
        )
        .offset(pending.offset as u64)
        .build()
        .user_data(slot as u64);
        unsafe { ring.submission().push(&entry) }.map_err(|_| io::Error::other("ring SQ full"))
    }

    struct RingReader {
        ring: IoUring,
        pending: Vec<Pending>,
    }
    impl RingReader {
        fn new(depth: usize) -> io::Result<Self> {
            Ok(Self {
                ring: IoUring::new(depth as u32)?,
                pending: (0..depth).map(|_| Pending::default()).collect(),
            })
        }
        fn read(
            &mut self,
            ctx: &Context,
            indices: &[usize],
            verify: bool,
        ) -> io::Result<ReadResults> {
            let depth = self.pending.len();
            let mut results = vec![(0, 0); indices.len()];
            for batch_start in (0..indices.len()).step_by(depth) {
                let batch = &indices[batch_start..indices.len().min(batch_start + depth)];
                for (slot, &request) in batch.iter().enumerate() {
                    let file = ctx.open(request)?;
                    let before = stamp(&file.metadata()?)?;
                    if before != ctx.requests[request].expected {
                        return Err(io::Error::other("candidate changed before open"));
                    }
                    if before.size > 128 * 1024 * 1024 {
                        return Err(io::Error::other("candidate exceeds benchmark buffer limit"));
                    }
                    let item = &mut self.pending[slot];
                    item.file = Some(file);
                    item.before = Some(before);
                    item.bytes.resize(before.size as usize, 0);
                    item.offset = 0;
                    item.request = request;
                }
                let mut active = 0;
                for (slot, item) in self.pending.iter_mut().take(batch.len()).enumerate() {
                    if item.bytes.is_empty() {
                        continue;
                    }
                    if push_read(&mut self.ring, slot, item).is_err() {
                        std::process::abort();
                    }
                    active += 1;
                }
                let mut completed = 0;
                while completed < active {
                    if self.ring.submit_and_wait(1).is_err() {
                        std::process::abort();
                    }
                    let events = self
                        .ring
                        .completion()
                        .map(|c| (c.user_data() as usize, c.result()))
                        .collect::<Vec<_>>();
                    for (slot, result) in events {
                        if slot >= batch.len() || result <= 0 {
                            std::process::abort();
                        }
                        let item = &mut self.pending[slot];
                        item.offset += result as usize;
                        if item.offset > item.bytes.len() {
                            std::process::abort();
                        }
                        if item.offset < item.bytes.len() {
                            if push_read(&mut self.ring, slot, item).is_err() {
                                std::process::abort();
                            }
                        } else {
                            completed += 1;
                        }
                    }
                }
                // No kernel request remains before any descriptor or buffer is released.
                for (position, item) in self.pending.iter_mut().take(batch.len()).enumerate() {
                    let after = stamp(&item.file.as_ref().unwrap().metadata()?)?;
                    if Some(after) != item.before || item.offset != item.bytes.len() {
                        return Err(io::Error::other("candidate changed during ring read"));
                    }
                    results[batch_start + position] =
                        (after.size, fingerprint(&item.bytes, verify));
                    debug_assert_eq!(item.request, batch[position]);
                    item.file = None;
                    item.before = None;
                    item.bytes.clear();
                }
            }
            Ok(results)
        }
    }

    fn cpu_nanos() -> u128 {
        let mut stamp = std::mem::MaybeUninit::<libc::timespec>::uninit();
        // SAFETY: clock_gettime initializes its output on success.
        assert_eq!(
            unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, stamp.as_mut_ptr()) },
            0
        );
        let stamp = unsafe { stamp.assume_init() };
        stamp.tv_sec as u128 * 1_000_000_000 + stamp.tv_nsec as u128
    }
    fn rss_kib() -> u64 {
        fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| {
                s.lines().find_map(|line| {
                    line.strip_prefix("VmHWM:")
                        .and_then(|v| v.split_whitespace().next())
                        .and_then(|v| v.parse().ok())
                })
            })
            .unwrap_or(0)
    }

    fn select_indices(
        picker: &FilePicker,
        count: usize,
        pattern: &str,
        kind: &str,
    ) -> (Vec<String>, String, usize) {
        let query = vec![pattern.to_string()];
        let selected = if kind == "selective" {
            picker.candidate_ids(Some(&query))
        } else {
            picker.candidate_ids(None)
        };
        let available = selected.len();
        let origin = if kind == "selective" && selected.len() >= count {
            "bigram-query"
        } else {
            "broad-index-subset"
        };
        let ids = if selected.len() <= count {
            selected
        } else if count == 1 || kind == "adjacent" || origin == "bigram-query" {
            selected.into_iter().take(count).collect()
        } else {
            (0..count)
                .map(|i| selected[i * (selected.len() - 1) / (count - 1)])
                .collect()
        };
        (
            ids.into_iter().map(str::to_owned).collect(),
            origin.to_owned(),
            available,
        )
    }

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let mut args = env::args().skip(1);
        let mut repo = None;
        let mut collection = None;
        let mut pattern = String::from("the");
        let mut encoding_override = None;
        let mut runs = 5;
        let mut workers = 16;
        let mut depths = vec![8, 32];
        let mut sizes = vec![32, 256, 1024, 4096];
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--repo" => repo = args.next().map(PathBuf::from),
                "--collection" => collection = args.next(),
                "--pattern" => pattern = args.next().ok_or("missing pattern")?,
                "--encoding" => {
                    encoding_override =
                        Some(match args.next().ok_or("missing encoding")?.as_str() {
                            "ascii" => BigramEncoding::Ascii,
                            "utf8_bytes" => BigramEncoding::Utf8Bytes,
                            _ => return Err("encoding must be ascii or utf8_bytes".into()),
                        })
                }
                "--runs" => runs = args.next().ok_or("missing runs")?.parse()?,
                "--workers" => workers = args.next().ok_or("missing workers")?.parse()?,
                "--depths" => {
                    depths = args
                        .next()
                        .ok_or("missing depths")?
                        .split(',')
                        .map(str::parse)
                        .collect::<Result<_, _>>()?
                }
                "--sizes" => {
                    sizes = args
                        .next()
                        .ok_or("missing sizes")?
                        .split(',')
                        .map(str::parse)
                        .collect::<Result<_, _>>()?
                }
                _ => return Err(format!("unknown argument: {arg}").into()),
            }
        }
        if runs == 0 || workers == 0 || depths.contains(&0) || sizes.contains(&0) {
            return Err("runs, workers, depths and sizes must be positive".into());
        }
        let repo = repo.ok_or("--repo required")?;
        let collection = collection.ok_or("--collection required")?;
        let model = Repository::open(&repo)?;
        let config = model
            .model()
            .collections
            .get(&collection)
            .ok_or("unknown collection")?;
        let encoding = encoding_override
            .or(config.bigram_encoding)
            .unwrap_or(BigramEncoding::Ascii);
        let picker = FilePicker::open_with_encoding(&config.path, encoding)?;
        let root_fd = open_root(&config.path)?;
        let root_stamp = dir_stamp(&root_fd.metadata()?)?;
        let mut context = Context {
            root: config.path.clone(),
            root_fd,
            root_stamp,
            requests: Vec::new(),
        };
        // Prepare all samples and expected stamps before any timed read.
        let mut samples = Vec::new();
        for kind in ["adjacent", "scattered", "selective"] {
            for &count in &sizes {
                let (ids, origin, available) = select_indices(&picker, count, &pattern, kind);
                let start = context.requests.len();
                for id in ids {
                    context.requests.push(Request {
                        id,
                        expected: Stamp {
                            size: 0,
                            dev: 0,
                            ino: 0,
                            mtime: (0, 0),
                            ctime: (0, 0),
                        },
                    });
                    let index = context.requests.len() - 1;
                    context.requests[index].expected = stamp(&context.open(index)?.metadata()?)?;
                }
                samples.push((
                    kind.to_owned(),
                    count,
                    origin,
                    available,
                    (start..context.requests.len()).collect::<Vec<_>>(),
                ));
            }
        }
        // The Compio worker owns a pinned directory and an immutable copy of the request table.
        let compio = CompioWorker::new(Arc::new(context.fork()?))?;
        let sync = SyncWorker::new(Arc::new(context.fork()?), workers.min(8))?;
        let mut rings = depths
            .iter()
            .map(|&depth| RingReader::new(depth).map(|reader| (depth, reader)))
            .collect::<io::Result<Vec<_>>>()?;
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .build()?;
        for (kind, nominal, origin, available, indices) in &samples {
            let mut size_buckets = [0_u64; 5];
            for &index in indices {
                size_buckets[match context.requests[index].expected.size {
                    0..=255 => 0,
                    256..=1023 => 1,
                    1024..=4095 => 2,
                    4096..=16383 => 3,
                    _ => 4,
                }] += 1;
            }
            let reference = rayon_reads(&context, indices, &pool, true)?;
            let pipeline = sync.read(indices, true)?;
            if reference != pipeline {
                return Err("sync pipeline content differs from Rayon".into());
            }
            for &depth in &depths {
                if reference != compio.read(indices, depth, true)? {
                    return Err("Compio content differs from Rayon".into());
                }
                let ring = &mut rings.iter_mut().find(|(d, _)| *d == depth).unwrap().1;
                if reference != ring.read(&context, indices, true)? {
                    return Err("ring content differs from Rayon".into());
                }
            }
            for round in 0..runs {
                let mut variants =
                    vec![("rayon", workers, 0), ("sync-pipeline", workers.min(8), 0)];
                for &depth in &depths {
                    variants.push(("compio", 1, depth));
                    variants.push(("io-uring", 1, depth));
                }
                if round % 2 == 1 {
                    variants.reverse();
                }
                for (backend, threads, depth) in variants {
                    let cpu = cpu_nanos();
                    let allocated_before = super::allocation::start();
                    let started = Instant::now();
                    let result = match backend {
                        "rayon" => rayon_reads(&context, indices, &pool, false)?,
                        "sync-pipeline" => sync.read(indices, false)?,
                        "compio" => compio.read(indices, depth, false)?,
                        "io-uring" => {
                            let ring = &mut rings.iter_mut().find(|(d, _)| *d == depth).unwrap().1;
                            ring.read(&context, indices, false)?
                        }
                        _ => unreachable!(),
                    };
                    let wall = started.elapsed().as_nanos();
                    let (allocations, allocated_bytes, peak_live_delta_bytes) =
                        super::allocation::finish(allocated_before);
                    let cpu = cpu_nanos() - cpu;
                    if result.iter().map(|v| v.0).collect::<Vec<_>>()
                        != reference.iter().map(|v| v.0).collect::<Vec<_>>()
                    {
                        return Err("timed read sizes differ".into());
                    }
                    context.check_root()?;
                    println!(
                        "{}",
                        json!({ "backend": backend, "repo": repo, "collection": collection,
                            "pattern": pattern, "sample": kind, "sample_origin": origin, "nominal_candidates": nominal,
                            "available_candidates": available, "encoding": format!("{encoding:?}"),
                            "size_buckets": size_buckets,
                            "candidates": indices.len(), "reads": result.len(), "bytes": result.iter().map(|v| v.0).sum::<u64>(),
                            "workers": workers, "io_threads": threads, "depth": depth, "round": round + 1,
                            "wall_nanos": wall, "cpu_nanos": cpu, "peak_rss_kib": rss_kib(),
                        "runtime_init_nanos": if backend == "compio" { compio.init_nanos } else if backend == "sync-pipeline" { sync.init_nanos } else { 0 },
                        "allocations": allocations, "allocated_bytes": allocated_bytes,
                        "peak_live_delta_bytes": peak_live_delta_bytes,
                            "content_verified": true,
                        })
                    );
                }
            }
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::os::unix::fs::symlink;

        fn fixture() -> (tempfile::TempDir, Context) {
            let dir = tempfile::tempdir().unwrap();
            fs::write(dir.path().join("a.json"), br#"{"text":"hello"}"#).unwrap();
            let root_fd = File::open(dir.path()).unwrap();
            let root_stamp = dir_stamp(&root_fd.metadata().unwrap()).unwrap();
            let mut ctx = Context {
                root: dir.path().to_path_buf(),
                root_fd,
                root_stamp,
                requests: vec![Request {
                    id: "a".into(),
                    expected: Stamp {
                        size: 0,
                        dev: 0,
                        ino: 0,
                        mtime: (0, 0),
                        ctime: (0, 0),
                    },
                }],
            };
            ctx.requests[0].expected = stamp(&ctx.open(0).unwrap().metadata().unwrap()).unwrap();
            (dir, ctx)
        }

        #[test]
        fn all_readers_return_identical_verified_bytes() {
            let (_dir, ctx) = fixture();
            let rayon = rayon::ThreadPoolBuilder::new()
                .num_threads(2)
                .build()
                .unwrap();
            let reference = rayon_reads(&ctx, &[0], &rayon, true).unwrap();
            let sync = SyncWorker::new(Arc::new(ctx.fork().unwrap()), 2).unwrap();
            let compio = CompioWorker::new(Arc::new(ctx.fork().unwrap())).unwrap();
            let mut ring = RingReader::new(8).unwrap();
            assert_eq!(reference, sync.read(&[0], true).unwrap());
            assert_eq!(reference, compio.read(&[0], 8, true).unwrap());
            assert_eq!(reference, ring.read(&ctx, &[0], true).unwrap());
        }

        #[test]
        fn changed_and_symlinked_files_are_rejected() {
            let (dir, ctx) = fixture();
            fs::write(dir.path().join("a.json"), br#"{"text":"changed"}"#).unwrap();
            assert!(sync_one(&ctx, 0, &mut Vec::new(), false).is_err());
            let compio = CompioWorker::new(Arc::new(ctx.fork().unwrap())).unwrap();
            assert!(compio.read(&[0], 8, false).is_err());
            let mut ring = RingReader::new(8).unwrap();
            assert!(ring.read(&ctx, &[0], false).is_err());

            let outside = tempfile::NamedTempFile::new().unwrap();
            fs::remove_file(dir.path().join("a.json")).unwrap();
            symlink(outside.path(), dir.path().join("a.json")).unwrap();
            assert!(ctx.open(0).is_err());
        }

        #[test]
        fn replaced_source_directory_is_detected() {
            let (dir, ctx) = fixture();
            let old = dir.path().with_extension("old");
            fs::rename(dir.path(), &old).unwrap();
            fs::create_dir(dir.path()).unwrap();
            assert!(ctx.check_root().is_err());
            fs::remove_dir(dir.path()).unwrap();
            fs::rename(&old, dir.path()).unwrap();
        }
    }
}

#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    linux::run()
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("candidate-io-benchmark requires Linux");
}
