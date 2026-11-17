use std::ffi::OsString;

use crate::archive_storage::ValidatedArchive;
use crate::archived_path::{BorrowedStoredPath, StoredPath, to_shared};
use crate::inventory::SharedPath;

#[test]
fn borrowed_path_adapter_round_trips_plain_unicode_and_escaped_paths() {
    let mut paths = vec![
        SharedPath::from("data/a.json"),
        SharedPath::from("data/über/ファイル.json"),
        SharedPath::from("data/%literal\\separator.json"),
    ];
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        paths.push(SharedPath::from(OsString::from_vec(
            b"data/non-utf8-\xff.json".to_vec(),
        )));
    }

    for path in paths {
        let owned = StoredPath::from_shared(&path).to_shared().unwrap();
        assert_eq!(owned, path);
        let archive =
            ValidatedArchive::<StoredPath>::freeze_source(&BorrowedStoredPath(&path)).unwrap();
        assert_eq!(to_shared(archive.root()).unwrap(), owned);
    }
}
