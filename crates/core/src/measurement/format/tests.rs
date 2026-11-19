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
