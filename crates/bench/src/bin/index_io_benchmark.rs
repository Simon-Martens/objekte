//! Linux-only, read-only filesystem experiments. Production I/O is unchanged.


#[cfg(target_os = "linux")]
mod linux {
    use io_uring::{IoUring, opcode, types};
    use objekte_core::Repository;
    use rayon::prelude::*;
    use serde_json::json;
    use std::{
        collections::BTreeMap,
        env,
        ffi::CString,
        fs::{self, File, Metadata},
        hash::{Hash, Hasher},
        io::{self, Read},
        mem::MaybeUninit,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::{ffi::OsStrExt, fs::MetadataExt},
        },
        path::{Path, PathBuf},
        time::Instant,
    };


    struct Directory {
        path: PathBuf,
        fd: File,
        stamp: (u64, u64, i64, i64),
    }


    fn dir_stamp(meta: &Metadata) -> (u64, u64, i64, i64) {
        (meta.dev(), meta.ino(), meta.mtime(), meta.mtime_nsec())
    }


    struct FileSpec {
        id: String,
        path: PathBuf,
        name: CString,
        directory: usize,
    }


    struct Inventory {
        files: Vec<FileSpec>,
        directories: Vec<Directory>,
        enumerate_nanos: u128,
        id_nanos: u128,
        sort_nanos: u128,
    }


    fn open_dir(parent: Option<&File>, path: &Path, name: Option<&CString>) -> io::Result<File> {
        let root_name;
        let (fd, name) = if let (Some(parent), Some(name)) = (parent, name) {
            (parent.as_raw_fd(), name)
        } else {
            root_name = CString::new(path.as_os_str().as_bytes())
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL path"))?;
            (libc::AT_FDCWD, &root_name)
        };
        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        // SAFETY: the path is NUL-terminated; this function owns a successful descriptor.
        let raw = unsafe { libc::openat(fd, name.as_ptr(), flags) };
        if raw < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(unsafe { File::from_raw_fd(raw) })
        }
    }

    fn discover(root: &Path, recursive: bool) -> io::Result<Inventory> {
        let root = fs::canonicalize(root)?;
        let meta = fs::symlink_metadata(&root)?;
        let mut directories = vec![Directory {
            path: root.clone(),
            fd: open_dir(None, &root, None)?,
            stamp: dir_stamp(&meta),
        }];
        let mut pending = vec![0_usize];
        let mut files = Vec::new();
        let mut enumerate_nanos = 0;
        let mut id_nanos = 0;
        while let Some(parent) = pending.pop() {
            let started = Instant::now();
            let entries =
                fs::read_dir(&directories[parent].path)?.collect::<io::Result<Vec<_>>>()?;
            enumerate_nanos += started.elapsed().as_nanos();
            let started = Instant::now();
            for entry in entries {
                let path = entry.path();
                let kind = entry.file_type()?;
                if kind.is_dir() {
                    if matches!(
                        entry.file_name().to_str(),
                        Some(".editor" | ".objekte" | ".git")
                    ) {
                        continue;
                    }
                    if recursive {
                        let name = CString::new(entry.file_name().as_bytes())
                            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "NUL name"))?;
                        let fd = open_dir(Some(&directories[parent].fd), &path, Some(&name))?;
                        let meta = fs::symlink_metadata(&path)?;
                        if !meta.is_dir() {
                            return Err(io::Error::other("directory replaced"));
                        }
                        directories.push(Directory {
                            path,
                            fd,
                            stamp: dir_stamp(&meta),
                        });
                        pending.push(directories.len() - 1);
                    }
                    continue;
                }
                if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                    continue;
                }
                if !kind.is_file() {
                    return Err(io::Error::other("JSON source is not regular"));
                }
                let relative = path
                    .strip_prefix(&root)
                    .expect("walk remains under root")
                    .with_extension("");
                let id = relative
                    .to_str()
                    .ok_or_else(|| io::Error::other("non-UTF8 ID"))?
                    .replace(std::path::MAIN_SEPARATOR, "/");
                if id.is_empty()
                    || id
                        .split('/')
                        .any(|part| part.is_empty() || part == "." || part == "..")
                {
                    return Err(io::Error::other("invalid ID"));
                }
                let name = CString::new(entry.file_name().as_bytes())
                    .map_err(|_| io::Error::other("NUL name"))?;
                files.push(FileSpec {
                    id,
                    path,
                    name,
                    directory: parent,
                });
            }
            id_nanos += started.elapsed().as_nanos();
        }
        let started = Instant::now();
        files.sort_unstable_by(|left, right| left.id.cmp(&right.id));
        if files.windows(2).any(|pair| pair[0].id == pair[1].id) {
            return Err(io::Error::other("duplicate ID"));
        }
        let sort_nanos = started.elapsed().as_nanos();
        Ok(Inventory {
            files,
            directories,
            enumerate_nanos,
            id_nanos,
            sort_nanos,
        })
    }


    #[derive(Clone, Copy, PartialEq, Eq, Hash)]
    struct StatKey {
        size: u64,
        modified: i128,
        changed: i128,
        inode: u64,
        major: u32,
        minor: u32,
    }


    fn std_key(m: &Metadata) -> io::Result<StatKey> {
        if !m.is_file() || m.len() > i64::MAX as u64 {
            return Err(io::Error::other("invalid source"));
        }
        Ok(StatKey {
            size: m.len(),
            modified: i128::from(m.mtime()) * 1_000_000_000 + i128::from(m.mtime_nsec()),
            changed: i128::from(m.ctime()) * 1_000_000_000 + i128::from(m.ctime_nsec()),
            inode: m.ino(),
            major: libc::major(m.dev()) as u32,
            minor: libc::minor(m.dev()) as u32,
        })
    }

    fn statx_key(m: &libc::statx) -> io::Result<StatKey> {
        if m.stx_mask & libc::STATX_BASIC_STATS != libc::STATX_BASIC_STATS
            || (u32::from(m.stx_mode) & libc::S_IFMT) != libc::S_IFREG
            || m.stx_size > i64::MAX as u64
        {
            return Err(io::Error::other("invalid statx source"));
        }
        Ok(StatKey {
            size: m.stx_size,
            modified: i128::from(m.stx_mtime.tv_sec) * 1_000_000_000
                + i128::from(m.stx_mtime.tv_nsec),
            changed: i128::from(m.stx_ctime.tv_sec) * 1_000_000_000
                + i128::from(m.stx_ctime.tv_nsec),
            inode: m.stx_ino,
            major: m.stx_dev_major,
            minor: m.stx_dev_minor,
        })
    }

    fn sync_statx(inventory: &Inventory, file: &FileSpec) -> io::Result<StatKey> {
        // SAFETY: the output is initialized, and the descriptor and name live for this call.
        let mut output: libc::statx = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::statx(
                inventory.directories[file.directory].fd.as_raw_fd(),
                file.name.as_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
                libc::STATX_BASIC_STATS,
                &mut output,
            )
        };
        if result != 0 {
            Err(io::Error::last_os_error())
        } else {
            statx_key(&output)
        }
    }

    fn ring_statx(inventory: &Inventory, depth: usize) -> io::Result<Vec<StatKey>> {
        let mut ring = IoUring::new(depth as u32)?;
        let mut outputs = (0..depth)
            .map(|_| Box::new(unsafe { std::mem::zeroed::<libc::statx>() }))
            .collect::<Vec<_>>();
        let mut result = vec![None; inventory.files.len()];
        for start in (0..inventory.files.len()).step_by(depth) {
            let batch = depth.min(inventory.files.len() - start);
            for (slot, output) in outputs.iter_mut().enumerate().take(batch) {
                let file = &inventory.files[start + slot];
                let entry = opcode::Statx::new(
                    types::Fd(inventory.directories[file.directory].fd.as_raw_fd()),
                    file.name.as_ptr(),
                    output.as_mut() as *mut libc::statx as *mut types::statx,
                )
                .flags(libc::AT_SYMLINK_NOFOLLOW)
                .mask(libc::STATX_BASIC_STATS)
                .build()
                .user_data(slot as u64);
                // SAFETY: request name, output, and dirfd remain alive until completion.
                unsafe { ring.submission().push(&entry) }
                    .map_err(|_| io::Error::other("SQ full"))?;
            }
            let mut completed = 0;
            while completed < batch {
                ring.submit_and_wait(batch - completed)
                    .unwrap_or_else(|error| ring_fatal(error));
                for event in &mut ring.completion() {
                    let slot = event.user_data() as usize;
                    if slot >= batch || result[start + slot].is_some() {
                        ring_fatal(io::Error::other("bad CQE"));
                    }
                    if event.result() < 0 {
                        ring_fatal(io::Error::from_raw_os_error(-event.result()));
                    }
                    result[start + slot] =
                        Some(statx_key(&outputs[slot]).unwrap_or_else(|error| ring_fatal(error)));
                    completed += 1;
                }
            }
        }
        result
            .into_iter()
            .map(|value| value.ok_or_else(|| io::Error::other("missing CQE")))
            .collect()
    }

    /// Once submissions are in flight, leaving the process is safer than
    /// unwinding and freeing memory that the kernel may still write into.
    fn ring_fatal(error: io::Error) -> ! {
        eprintln!("io_uring benchmark failed with in-flight requests: {error}");
        std::process::exit(2)
    }

    fn open_file(inventory: &Inventory, file: &FileSpec) -> io::Result<File> {
        let flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK;
        // SAFETY: the parent directory is pinned and the final name is NUL-terminated.
        let raw = unsafe {
            libc::openat(
                inventory.directories[file.directory].fd.as_raw_fd(),
                file.name.as_ptr(),
                flags,
            )
        };
        if raw < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(unsafe { File::from_raw_fd(raw) })
        }
    }

    fn read_sync(inventory: &Inventory, workers: usize) -> io::Result<(u64, u64)> {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .build()
            .map_err(io::Error::other)?;
        pool.install(|| {
            inventory
                .files
                .par_iter()
                .map_init(Vec::new, |buffer, spec| {
                    let mut file = open_file(inventory, spec)?;
                    let before = std_key(&file.metadata()?)?;
                    buffer.clear();
                    file.read_to_end(buffer)?;
                    let after = std_key(&file.metadata()?)?;
                    if before != after || buffer.len() as u64 != before.size {
                        return Err(io::Error::other("file changed while reading"));
                    }
                    Ok(before.size)
                })
                .try_fold(
                    || (0_u64, 0_u64),
                    |(count, bytes), value: io::Result<u64>| Ok((count + 1, bytes + value?)),
                )
                .try_reduce(|| (0, 0), |a, b| Ok((a.0 + b.0, a.1 + b.1)))
        })
    }


    #[derive(Default)]
    struct RingReadSlot {
        file: Option<File>,
        before: Option<StatKey>,
        buffer: Vec<MaybeUninit<u8>>,
        offset: usize,
        reading: bool,
    }


    fn push_open(
        ring: &mut IoUring,
        inventory: &Inventory,
        slot: usize,
        index: usize,
    ) -> io::Result<()> {
        let spec = &inventory.files[index];
        let entry = opcode::OpenAt::new(
            types::Fd(inventory.directories[spec.directory].fd.as_raw_fd()),
            spec.name.as_ptr(),
        )
        .flags(libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .build()
        .user_data(slot as u64);
        // SAFETY: the pathname and directory descriptor live until the CQE arrives.
        unsafe { ring.submission().push(&entry) }.map_err(|_| io::Error::other("open SQ full"))
    }

    fn push_read(ring: &mut IoUring, slot: usize, state: &mut RingReadSlot) -> io::Result<()> {
        let remaining = state.buffer.len() - state.offset;
        let len = u32::try_from(remaining)
            .map_err(|_| io::Error::other("file too large for ring read"))?;
        // SAFETY: offset is within the owned vector; it remains allocated until this CQE.
        let pointer = unsafe { state.buffer.as_mut_ptr().add(state.offset) as *mut u8 };
        let entry = opcode::Read::new(
            types::Fd(state.file.as_ref().unwrap().as_raw_fd()),
            pointer,
            len,
        )
        .offset(state.offset as u64)
        .build()
        .user_data(slot as u64);
        // SAFETY: the file descriptor and buffer remain owned by this slot until completion.
        unsafe { ring.submission().push(&entry) }.map_err(|_| io::Error::other("read SQ full"))?;
        state.reading = true;
        Ok(())
    }

    fn read_ring(inventory: &Inventory, depth: usize) -> io::Result<(u64, u64, u64)> {
        let mut ring = IoUring::new(depth as u32)?;
        let mut slots = (0..depth)
            .map(|_| RingReadSlot::default())
            .collect::<Vec<_>>();
        let mut next = 0;
        let mut completed = 0_u64;
        let mut bytes = 0_u64;
        let mut read_ops = 0_u64;
        for slot in 0..slots.len().min(inventory.files.len()) {
            push_open(&mut ring, inventory, slot, next)?;
            next += 1;
        }
        while completed < inventory.files.len() as u64 {
            ring.submit_and_wait(1)
                .unwrap_or_else(|error| ring_fatal(error));
            let events = ring
                .completion()
                .map(|event| (event.user_data() as usize, event.result()))
                .collect::<Vec<_>>();
            for (slot, result) in events {
                if slot >= slots.len() {
                    ring_fatal(io::Error::other("invalid read CQE"));
                }
                if result < 0 {
                    ring_fatal(io::Error::from_raw_os_error(-result));
                }
                let state = &mut slots[slot];
                if !state.reading {
                    state.file = Some(unsafe { File::from_raw_fd(result) });
                    let before = std_key(
                        &state
                            .file
                            .as_ref()
                            .unwrap()
                            .metadata()
                            .unwrap_or_else(|error| ring_fatal(error)),
                    )
                    .unwrap_or_else(|error| ring_fatal(error));
                    let len = usize::try_from(before.size)
                        .unwrap_or_else(|_| ring_fatal(io::Error::other("file too large")));
                    if len > 128 * 1024 * 1024 {
                        ring_fatal(io::Error::other("read-only benchmark file exceeds 128 MiB"));
                    }
                    state.before = Some(before);
                    state.buffer.resize_with(len, MaybeUninit::uninit);
                    state.offset = 0;
                    if len > 0 {
                        push_read(&mut ring, slot, state).unwrap_or_else(|error| ring_fatal(error));
                        read_ops += 1;
                        continue;
                    }
                } else {
                    if result == 0 {
                        ring_fatal(io::Error::other("short read reached EOF"));
                    }
                    state.offset += result as usize;
                    if state.offset < state.buffer.len() {
                        push_read(&mut ring, slot, state).unwrap_or_else(|error| ring_fatal(error));
                        read_ops += 1;
                        continue;
                    }
                }
                let after = std_key(
                    &state
                        .file
                        .as_ref()
                        .unwrap()
                        .metadata()
                        .unwrap_or_else(|error| ring_fatal(error)),
                )
                .unwrap_or_else(|error| ring_fatal(error));
                if Some(after) != state.before {
                    ring_fatal(io::Error::other("file changed while reading"));
                }
                bytes += after.size;
                completed += 1;
                state.file = None;
                state.before = None;
                state.buffer.clear();
                state.reading = false;
                if next < inventory.files.len() {
                    push_open(&mut ring, inventory, slot, next)
                        .unwrap_or_else(|error| ring_fatal(error));
                    next += 1;
                }
            }
        }
        Ok((completed, bytes, read_ops))
    }

    fn cpu_nanos() -> u128 {
        let mut stamp: libc::timespec = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut stamp) },
            0
        );
        stamp.tv_sec as u128 * 1_000_000_000 + stamp.tv_nsec as u128
    }

    fn proc_value(key: &str) -> Option<u64> {
        fs::read_to_string("/proc/self/status")
            .ok()?
            .lines()
            .find_map(|line| {
                line.strip_prefix(key)
                    .and_then(|rest| rest.split_whitespace().next())
                    .and_then(|text| text.parse().ok())
            })
    }

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let mut args = env::args().skip(1);
        let mut repo = None;
        let mut collection = None;
        let mut backend = String::from("rayon-metadata");
        let mut workers = 32;
        let mut depth = 64;
        let mut verify_metadata = false;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--repo" => repo = args.next().map(PathBuf::from),
                "--collection" => collection = args.next(),
                "--backend" => backend = args.next().ok_or("missing backend")?,
                "--workers" => workers = args.next().ok_or("missing workers")?.parse()?,
                "--depth" => depth = args.next().ok_or("missing depth")?.parse()?,
                "--verify-metadata" => verify_metadata = true,
                _ => return Err(format!("unknown argument {arg}").into()),
            }
        }
        let repo = repo.ok_or("--repo required")?;
        let collection = collection.ok_or("--collection required")?;
        let model = Repository::open(&repo)?;
        let config = model
            .model()
            .collections
            .get(&collection)
            .ok_or("unknown collection")?;
        let inventory = discover(&config.path, config.recursive)?;
        if backend == "read-rayon" || backend == "read-uring" {
            let cpu_start = cpu_nanos();
            let started = Instant::now();
            let (count, bytes, read_ops) = if backend == "read-rayon" {
                let (count, bytes) = read_sync(&inventory, workers)?;
                (count, bytes, count)
            } else {
                read_ring(&inventory, depth)?
            };
            let read_nanos = started.elapsed().as_nanos();
            if count != inventory.files.len() as u64 {
                return Err("read count mismatch".into());
            }
            for directory in &inventory.directories {
                if dir_stamp(&fs::symlink_metadata(&directory.path)?) != directory.stamp {
                    return Err("directory changed during read benchmark".into());
                }
            }
            println!(
                "{}",
                json!({
                    "backend": backend, "repo": repo, "collection": collection,
                    "files": count, "bytes": bytes, "workers": workers, "queue_depth": depth,
                    "directory_enumeration_nanos": inventory.enumerate_nanos,
                    "id_construction_nanos": inventory.id_nanos, "sort_nanos": inventory.sort_nanos,
                    "read_nanos": read_nanos, "read_cpu_nanos": cpu_nanos() - cpu_start,
                    "open_ops": count, "metadata_ops": count * 2,
                    "read_ops": read_ops, "close_ops": count,
                    "peak_rss_kib": proc_value("VmHWM:"),
                    "voluntary_context_switches": proc_value("voluntary_ctxt_switches:"),
                    "nonvoluntary_context_switches": proc_value("nonvoluntary_ctxt_switches:"),
                })
            );
            return Ok(());
        }
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .build()?;
        let cpu_start = cpu_nanos();
        let started = Instant::now();
        let keys: Vec<StatKey> = match backend.as_str() {
            "rayon-metadata" => pool.install(|| {
                inventory
                    .files
                    .par_iter()
                    .map(|file| std_key(&fs::symlink_metadata(&file.path)?))
                    .collect::<io::Result<_>>()
            })?,
            "sync-statx" => pool.install(|| {
                inventory
                    .files
                    .par_iter()
                    .map(|file| sync_statx(&inventory, file))
                    .collect::<io::Result<_>>()
            })?,
            "uring-statx" => ring_statx(&inventory, depth)?,
            _ => return Err(format!("unknown backend {backend}").into()),
        };
        let metadata_nanos = started.elapsed().as_nanos();
        let metadata_cpu_nanos = cpu_nanos() - cpu_start;
        if verify_metadata {
            for (file, key) in inventory.files.iter().zip(&keys) {
                let expected = std_key(&fs::symlink_metadata(&file.path)?)?;
                if *key != expected {
                    return Err(format!("metadata differs for {}", file.id).into());
                }
            }
        }
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for (file, key) in inventory.files.iter().zip(&keys) {
            file.id.hash(&mut hasher);
            key.hash(&mut hasher);
        }
        let metadata_fingerprint = hasher.finish();
        let started = Instant::now();
        let mut stamps = BTreeMap::new();
        let mut total_bytes = 0_u64;
        let mut size_buckets = [0_u64; 5];
        for (file, key) in inventory.files.iter().zip(&keys) {
            total_bytes += key.size;
            size_buckets[match key.size {
                0..=255 => 0,
                256..=1023 => 1,
                1024..=4095 => 2,
                4096..=16383 => 3,
                _ => 4,
            }] += 1;
            stamps.insert(
                &file.id,
                (
                    key.size,
                    key.modified.to_string(),
                    key.changed.to_string(),
                    key.inode,
                ),
            );
        }
        let stamp_map_nanos = started.elapsed().as_nanos();
        let started = Instant::now();
        for directory in &inventory.directories {
            if dir_stamp(&fs::symlink_metadata(&directory.path)?) != directory.stamp {
                return Err("directory changed during benchmark".into());
            }
        }
        let validation_nanos = started.elapsed().as_nanos();
        if stamps.len() != keys.len() {
            return Err("metadata count mismatch".into());
        }
        println!(
            "{}",
            json!({
                "backend": backend, "repo": repo, "collection": collection,
                "files": inventory.files.len(), "directories": inventory.directories.len(),
                "workers": workers, "queue_depth": depth,
                "directory_enumeration_nanos": inventory.enumerate_nanos,
                "id_construction_nanos": inventory.id_nanos, "sort_nanos": inventory.sort_nanos,
                "metadata_nanos": metadata_nanos, "metadata_cpu_nanos": metadata_cpu_nanos,
                "metadata_fingerprint": metadata_fingerprint,
                "stamp_and_map_nanos": stamp_map_nanos, "validation_nanos": validation_nanos,
                "metadata_ops": inventory.files.len() + 2 * inventory.directories.len(),


                "bytes": total_bytes, "size_buckets": size_buckets,
                "peak_rss_kib": proc_value("VmHWM:"),
                "voluntary_context_switches": proc_value("voluntary_ctxt_switches:"),




                "nonvoluntary_context_switches": proc_value("nonvoluntary_ctxt_switches:"),
            })
        );
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    linux::run()
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("index-io-benchmark requires Linux");
}
