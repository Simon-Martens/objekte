//! A clock that is absent from ordinary binaries. Call sites which collect
//! detailed query attribution retain their shape without reading time unless
//! the profiling feature is compiled in.

use std::time::{Duration, Instant};


#[derive(Clone, Copy)]
pub(crate) struct DetailedInstant {
    #[cfg(feature = "detailed-profiling")]
    started: Instant,
}


impl DetailedInstant {
    #[inline(always)]
    pub(crate) fn now() -> Self {
        Self {
            #[cfg(feature = "detailed-profiling")]
            started: Instant::now(),
        }
    }

    #[inline(always)]
    pub(crate) fn elapsed(&self) -> Duration {
        #[cfg(feature = "detailed-profiling")]
        {
            self.started.elapsed()
        }

        #[cfg(not(feature = "detailed-profiling"))]
        {
            Duration::ZERO
        }
    }

    #[inline(always)]
    pub(crate) fn duration_since(self, earlier: Self) -> Duration {
        #[cfg(feature = "detailed-profiling")]
        {
            self.started.duration_since(earlier.started)
        }

        #[cfg(not(feature = "detailed-profiling"))]
        {
            let _ = earlier;
            Duration::ZERO
        }
    }

    #[inline(always)]
    pub(crate) fn duration_since_instant(self, earlier: Instant) -> Duration {
        #[cfg(feature = "detailed-profiling")]
        {
            self.started.duration_since(earlier)
        }

        #[cfg(not(feature = "detailed-profiling"))]
        {
            let _ = earlier;
            Duration::ZERO
        }
    }
}


#[inline(always)]
pub(crate) fn start_if_enabled(enabled: bool) -> Option<std::time::Instant> {
    #[cfg(feature = "detailed-profiling")]
    {
        enabled.then(Instant::now)
    }

    #[cfg(not(feature = "detailed-profiling"))]
    {
        let _ = enabled;
        None
    }
}


#[cfg(all(test, not(feature = "detailed-profiling")))]
mod tests {
    use super::*;

    #[test]
    fn ordinary_build_has_no_detailed_clock_or_timer_state() {
        assert_eq!(std::mem::size_of::<DetailedInstant>(), 0);
        assert!(start_if_enabled(true).is_none());
        assert_eq!(DetailedInstant::now().elapsed(), Duration::ZERO);
        assert!(
            !crate::repository::RepositoryOptions {
                profile_build: true,
                ..Default::default()
            }
            .detailed_profiling_enabled()
        );
    }
}
