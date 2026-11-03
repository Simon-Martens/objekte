//! Optional index-build stage timers for profiling without changing the index.

use serde::Serialize;
use std::{ops::AddAssign, time::Instant};


/// Summed elapsed and on-thread CPU nanoseconds for one stage.
#[derive(Clone, Copy, Default, Serialize)]
pub struct StageMeasurement {
    pub wall_nanos: u64,
    pub cpu_nanos: u64,
}


impl AddAssign for StageMeasurement {
    fn add_assign(&mut self, other: Self) {
        self.wall_nanos = self.wall_nanos.saturating_add(other.wall_nanos);
        self.cpu_nanos = self.cpu_nanos.saturating_add(other.cpu_nanos);
    }
}


/// Timer local to the worker running a build stage.
pub(super) struct StageTimer {
    wall: Instant,
    cpu: u64,
}


impl StageTimer {
    pub(super) fn start() -> Self {
        Self {
            wall: Instant::now(),
            cpu: thread_cpu_nanos(),
        }
    }

    pub(super) fn stop(self) -> StageMeasurement {
        StageMeasurement {
            wall_nanos: self.wall.elapsed().as_nanos().min(u64::MAX as u128) as u64,
            cpu_nanos: thread_cpu_nanos().saturating_sub(self.cpu),
        }
    }
}


#[cfg(unix)]
fn thread_cpu_nanos() -> u64 {
    let stamp = rustix::time::clock_gettime(rustix::time::ClockId::ThreadCPUTime);
    (stamp.tv_sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(stamp.tv_nsec as u64)
}

#[cfg(not(unix))]
fn thread_cpu_nanos() -> u64 {
    0
}


/// Source projection stages summed across all scan workers.
#[derive(Clone, Copy, Default)]
pub(super) struct ProjectionStages {
    pub read: StageMeasurement,
    pub parse: StageMeasurement,
    pub values: StageMeasurement,
    pub field_text: StageMeasurement,
}


/// Source projection and posting insertion stages for one worker chunk.
#[derive(Clone, Copy, Default)]
pub(super) struct WorkerStages {
    pub projection: ProjectionStages,
    pub main_insert: StageMeasurement,
    pub rendered_insert: StageMeasurement,
    pub field_insert: StageMeasurement,
}


impl AddAssign for WorkerStages {
    fn add_assign(&mut self, other: Self) {
        self.projection.read += other.projection.read;
        self.projection.parse += other.projection.parse;
        self.projection.values += other.projection.values;
        self.projection.field_text += other.projection.field_text;
        self.main_insert += other.main_insert;
        self.rendered_insert += other.rendered_insert;
        self.field_insert += other.field_insert;
    }
}


/// Wall stages and summed worker time recorded while constructing a picker.
#[derive(Clone, Copy, Default, Serialize)]
pub struct BuildTimings {
    pub workers: usize,
    pub inventory_nanos: u64,
    pub slots_nanos: u64,
    pub scan_nanos: u64,
    pub project_worker_nanos: u64,
    pub add_worker_nanos: u64,
    pub source_read: StageMeasurement,
    pub json_parse: StageMeasurement,
    pub general_values: StageMeasurement,
    pub field_text: StageMeasurement,
    pub main_insert: StageMeasurement,
    pub rendered_insert: StageMeasurement,
    pub field_insert: StageMeasurement,
    pub compress_nanos: u64,
    pub main_compress: StageMeasurement,
    pub rendered_compress: StageMeasurement,
    pub field_compress: StageMeasurement,
    pub text_compress_nanos: u64,
    pub typed_nanos: u64,
    pub view_nanos: u64,
}


/// Compressed posting bytes assigned to one declared full-text path.
#[derive(Serialize)]
pub struct FieldIndexSize {
    pub path: String,
    pub normal_bytes: usize,
    pub rendered_bytes: usize,
}


/// Compressed posting sizes, excluding inventory and other secondary indexes.
#[derive(Serialize)]
pub struct IndexSizes {
    pub main_bytes: usize,
    pub rendered_global_bytes: usize,
    pub fields: Vec<FieldIndexSize>,
}


/// Wall-clock stages of early snapshot restoration. Overlapping phases are not additive.
#[derive(Clone, Copy, Default, Serialize)]
pub struct RestoreTimings {
    pub base_checksum_nanos: u64,
    pub manifest_decode_nanos: u64,
    pub base_filter_decode_nanos: u64,
    pub manifest_validation_nanos: u64,
    pub slot_restore_nanos: u64,
    pub secondary_structure_decode_nanos: u64,
    pub secondary_filter_decode_nanos: u64,
    pub secondary_validation_nanos: u64,
    pub secondary_checksum_nanos: u64,
    pub restore_total_nanos: u64,
    pub snapshot_bytes_read: u64,
    pub restored_files: usize,
    pub posting_columns: usize,
}
