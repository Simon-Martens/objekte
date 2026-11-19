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
