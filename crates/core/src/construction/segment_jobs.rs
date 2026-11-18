//! Bounded segment finalization and test-visible worker gates.

pub(crate) struct SegmentJobGate {
    state: std::sync::Mutex<SegmentJobGateState>,
    changed: std::sync::Condvar,
    #[cfg(test)]
    private_drain: std::sync::Mutex<PrivateDrainTestState>,
    #[cfg(test)]
    private_drain_changed: std::sync::Condvar,
}

#[derive(Default)]
struct SegmentJobGateState {
    workers: usize,
    release_workers: bool,
    post_submission: usize,
    release_post_submission: bool,
    post_validation: usize,
    release_post_validation: bool,
}

#[cfg(test)]
#[derive(Default)]
pub(super) struct PrivateDrainTestState {
    next_job: usize,
    failing_job: Option<usize>,
    blocked: std::collections::HashSet<usize>,
    arrived: std::collections::HashSet<usize>,
    released: std::collections::HashSet<usize>,
    completions: usize,
    errors: usize,
    final_drain_entries: usize,
    final_outstanding: Option<usize>,
}

#[allow(dead_code)]
impl SegmentJobGate {
    pub(crate) fn new() -> Self {
        Self {
            state: std::sync::Mutex::new(SegmentJobGateState::default()),
            changed: std::sync::Condvar::new(),
            #[cfg(test)]
            private_drain: std::sync::Mutex::new(PrivateDrainTestState::default()),
            #[cfg(test)]
            private_drain_changed: std::sync::Condvar::new(),
        }
    }

    pub(super) fn wait(&self) {
        let mut state = self.state.lock().expect("segment gate lock poisoned");
        state.workers += 1;
        self.changed.notify_all();
        while !state.release_workers {
            state = self
                .changed
                .wait(state)
                .expect("segment gate lock poisoned");
        }
    }

    pub(crate) fn wait_for_workers(&self, count: usize) {
        let mut state = self.state.lock().expect("segment gate lock poisoned");
        while state.workers < count {
            state = self
                .changed
                .wait(state)
                .expect("segment gate lock poisoned");
        }
    }

    pub(crate) fn release(&self) {
        let mut state = self.state.lock().expect("segment gate lock poisoned");
        state.release_workers = true;
        self.changed.notify_all();
    }

    pub(super) fn wait_after_validation(&self) {
        let mut state = self.state.lock().expect("segment gate lock poisoned");
        state.post_validation += 1;
        self.changed.notify_all();
        while !state.release_post_validation {
            state = self
                .changed
                .wait(state)
                .expect("segment gate lock poisoned");
        }
    }

    pub(super) fn wait_after_submission(&self) {
        let mut state = self.state.lock().expect("segment gate lock poisoned");
        state.post_submission += 1;
        self.changed.notify_all();
        while !state.release_post_submission {
            state = self
                .changed
                .wait(state)
                .expect("segment gate lock poisoned");
        }
    }

    pub(crate) fn wait_for_post_submission(&self) {
        let mut state = self.state.lock().expect("segment gate lock poisoned");
        while state.post_submission == 0 {
            state = self
                .changed
                .wait(state)
                .expect("segment gate lock poisoned");
        }
    }

    pub(crate) fn release_after_submission(&self) {
        let mut state = self.state.lock().expect("segment gate lock poisoned");
        state.release_post_submission = true;
        self.changed.notify_all();
    }

    pub(crate) fn wait_for_post_validation(&self) {
        let mut state = self.state.lock().expect("segment gate lock poisoned");
        while state.post_validation == 0 {
            state = self
                .changed
                .wait(state)
                .expect("segment gate lock poisoned");
        }
    }

    pub(crate) fn release_after_validation(&self) {
        let mut state = self.state.lock().expect("segment gate lock poisoned");
        state.release_post_validation = true;
        self.changed.notify_all();
    }

    #[cfg(test)]
    pub(crate) fn configure_private_final_drain_failure(
        &self,
        failing_job: usize,
        blocked_jobs: impl IntoIterator<Item = usize>,
    ) {
        let mut state = self
            .private_drain
            .lock()
            .expect("private drain gate lock poisoned");
        state.failing_job = Some(failing_job);
        state.blocked = blocked_jobs.into_iter().collect();
        state.released.clear();
        self.private_drain_changed.notify_all();
    }

    #[cfg(test)]
    pub(super) fn allocate_private_job(&self) -> usize {
        let mut state = self
            .private_drain
            .lock()
            .expect("private drain gate lock poisoned");
        let job = state.next_job;
        state.next_job += 1;
        job
    }

    #[cfg(test)]
    pub(super) fn wait_before_private_completion(&self, job: usize) {
        let mut state = self
            .private_drain
            .lock()
            .expect("private drain gate lock poisoned");
        state.arrived.insert(job);
        self.private_drain_changed.notify_all();
        while state.blocked.contains(&job) && !state.released.contains(&job) {
            state = self
                .private_drain_changed
                .wait(state)
                .expect("private drain gate lock poisoned");
        }
    }

    #[cfg(test)]
    pub(super) fn private_job_fails(&self, job: usize) -> bool {
        self.private_drain
            .lock()
            .expect("private drain gate lock poisoned")
            .failing_job
            == Some(job)
    }

    #[cfg(test)]
    pub(super) fn record_private_completion(&self, error: bool) {
        let mut state = self
            .private_drain
            .lock()
            .expect("private drain gate lock poisoned");
        state.completions += 1;
        state.errors += usize::from(error);
        self.private_drain_changed.notify_all();
    }

    #[cfg(test)]
    pub(super) fn record_private_final_drain(&self) {
        let mut state = self
            .private_drain
            .lock()
            .expect("private drain gate lock poisoned");
        state.final_drain_entries += 1;
        self.private_drain_changed.notify_all();
    }

    #[cfg(test)]
    pub(super) fn record_private_drain_complete(&self, outstanding: usize) {
        let mut state = self
            .private_drain
            .lock()
            .expect("private drain gate lock poisoned");
        state.final_outstanding = Some(outstanding);
        self.private_drain_changed.notify_all();
    }

    #[cfg(test)]
    pub(crate) fn release_private_job(&self, job: usize) {
        let mut state = self
            .private_drain
            .lock()
            .expect("private drain gate lock poisoned");
        state.released.insert(job);
        self.private_drain_changed.notify_all();
    }

    #[cfg(test)]
    pub(crate) fn release_all_private_jobs(&self) {
        let mut state = self
            .private_drain
            .lock()
            .expect("private drain gate lock poisoned");
        let blocked = state.blocked.clone();
        state.released.extend(blocked);
        self.private_drain_changed.notify_all();
    }

    #[cfg(test)]
    pub(crate) fn wait_for_private_job(&self, job: usize, timeout: std::time::Duration) -> bool {
        self.wait_for_private_state(timeout, |state| state.arrived.contains(&job))
    }

    #[cfg(test)]
    pub(crate) fn wait_for_private_final_drain(&self, timeout: std::time::Duration) -> bool {
        self.wait_for_private_state(timeout, |state| state.final_drain_entries != 0)
    }

    #[cfg(test)]
    pub(crate) fn wait_for_private_error(&self, timeout: std::time::Duration) -> bool {
        self.wait_for_private_state(timeout, |state| state.errors != 0)
    }

    #[cfg(test)]
    pub(super) fn wait_for_private_state(
        &self,
        timeout: std::time::Duration,
        ready: impl Fn(&PrivateDrainTestState) -> bool,
    ) -> bool {
        let started = std::time::Instant::now();
        let mut state = self
            .private_drain
            .lock()
            .expect("private drain gate lock poisoned");
        while !ready(&state) {
            let Some(remaining) = timeout.checked_sub(started.elapsed()) else {
                return false;
            };
            let (next, timed_out) = self
                .private_drain_changed
                .wait_timeout(state, remaining)
                .expect("private drain gate lock poisoned");
            state = next;
            if timed_out.timed_out() && !ready(&state) {
                return false;
            }
        }
        true
    }

    #[cfg(test)]
    pub(crate) fn private_drain_snapshot(&self) -> (usize, usize, Option<usize>) {
        let state = self
            .private_drain
            .lock()
            .expect("private drain gate lock poisoned");
        (state.completions, state.errors, state.final_outstanding)
    }
}
