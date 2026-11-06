use super::*;
#[cfg(unix)]
#[test]
fn invalid_bytes_and_literal_percent_remain_distinct() {
    use std::os::unix::ffi::OsStrExt;
    let native = Path::new(std::ffi::OsStr::from_bytes(b"a/\xFF%20\\.json"));
    assert_eq!(query_path(native), "a/%FF%2520%5C.json");
    assert_eq!(query_path(Path::new("a/ü.json")), "a/ü.json");
}
