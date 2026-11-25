use super::*;
#[cfg(unix)]
#[test]
fn invalid_bytes_and_literal_percent_remain_distinct() {
    use std::os::unix::ffi::OsStrExt;
    let native = Path::new(std::ffi::OsStr::from_bytes(b"a/\xFF%20\\.row"));
    assert_eq!(query_path(native), "a/%FF%2520%5C.row");
    assert_eq!(query_path(Path::new("a/ü.row")), "a/ü.row");
}
