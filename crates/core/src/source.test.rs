use super::*;
use std::{io::Read, os::unix::fs::symlink};

#[test]
fn pinned_directory_survives_replacement_but_finish_rejects_it() {
    let temp = tempfile::tempdir().unwrap();
    let p = temp.path().join("source");
    fs::create_dir(&p).unwrap();
    fs::write(p.join("a.json"), "old").unwrap();
    let source = Source::open(&p).unwrap();
    fs::rename(&p, temp.path().join("old-source")).unwrap();
    fs::create_dir(&p).unwrap();
    fs::write(p.join("a.json"), "new").unwrap();
    let mut text = String::new();
    source.file("a").unwrap().read_to_string(&mut text).unwrap();
    assert_eq!(text, "old");
    assert!(source.finish().is_err());
}

#[test]
fn symlink_swapped_in_after_inventory_is_never_followed() {
    let temp = tempfile::tempdir().unwrap();
    let p = temp.path().join("source");
    fs::create_dir(&p).unwrap();
    fs::write(p.join("a.json"), "original").unwrap();
    let source = Source::open(&p).unwrap();
    source.inventory().unwrap();
    let external = temp.path().join("external.json");
    fs::write(&external, "external").unwrap();
    fs::remove_file(p.join("a.json")).unwrap();
    symlink(&external, p.join("a.json")).unwrap();
    assert!(source.file("a").is_err());
    assert!(source.finish().is_err());
}

#[test]
fn membership_changes_after_inventory_are_detected() {
    let temp = tempfile::tempdir().unwrap();
    let source = Source::open(temp.path()).unwrap();
    source.inventory().unwrap();
    fs::write(temp.path().join("new.json"), "{}").unwrap();
    assert!(source.finish().is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn relative_stat_preserves_manifest_stamps_including_pre_epoch_timestamps() {
    let temp = tempfile::tempdir().unwrap();
    let p = temp.path().join("unicode-é.json");
    fs::write(&p, "{}").unwrap();
    for modified in [
        UNIX_EPOCH + std::time::Duration::new(1234, 567890123),
        UNIX_EPOCH - std::time::Duration::new(1, 234567890),
    ] {
        File::options()
            .write(true)
            .open(&p)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();
        let source = Source::open(temp.path()).unwrap();
        let inventory = source.inventory().unwrap();
        assert_eq!(
            inventory["unicode-é"],
            Stamp::from_metadata(&fs::symlink_metadata(&p).unwrap(), &p).unwrap()
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn relative_stat_rejects_a_symlink_even_when_its_target_is_regular() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("real.json"), "{}").unwrap();
    symlink("real.json", temp.path().join("link.json")).unwrap();
    let source = Source::open(temp.path()).unwrap();
    let stat = rustix::fs::statat(
        &source.directory,
        "link.json",
        rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
    )
    .unwrap();
    assert!(Stamp::from_stat(&stat, &temp.path().join("link.json")).is_err());
}
