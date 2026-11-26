// Expand each suite in its original module so it can access private items.
macro_rules! format_tests {
    () => {
        use super::{milliseconds_3, signed_milliseconds_3};

        #[test]
        fn durations_have_millisecond_units_and_three_decimals() {
            assert_eq!(milliseconds_3(0), "0.000");
            assert_eq!(milliseconds_3(499), "0.000");
            assert_eq!(milliseconds_3(500), "0.001");
            assert_eq!(milliseconds_3(1_234_567), "1.235");
            assert_eq!(milliseconds_3(2_000_000), "2.000");
            assert_eq!(signed_milliseconds_3(-1_234_567), "-1.235");
        }
    };
}
pub(super) use format_tests;

#[cfg(all(test, not(feature = "detailed-profiling")))]
macro_rules! timer_tests {
    () => {
        use super::*;

        #[test]
        fn ordinary_build_has_no_detailed_clock_or_timer_state() {
            assert_eq!(std::mem::size_of::<DetailedInstant>(), 0);
            assert!(start_if_enabled(true).is_none());
            assert_eq!(DetailedInstant::now().elapsed(), Duration::ZERO);
            assert!(
                !crate::repository::configuration::RepositoryOptions {
                    profile_build: true,
                    ..Default::default()
                }
                .detailed_profiling_enabled()
            );
        }
    };
}
#[cfg(all(test, not(feature = "detailed-profiling")))]
pub(super) use timer_tests;
