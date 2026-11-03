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

#[test]
fn recursive_inventory_uses_relative_ids_and_flat_inventory_stays_local() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("part/deeper")).unwrap();
    fs::write(temp.path().join("a.json"), "{}").unwrap();
    fs::write(temp.path().join("part/a.json"), "{}").unwrap();
    fs::write(temp.path().join("part/deeper/a.json"), "{}").unwrap();
    let flat = Source::open(temp.path()).unwrap();
    assert_eq!(
        flat.inventory()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["a"]
    );
    assert!(flat.file("part/a").is_err());
    let nested = Source::open_scoped(temp.path(), true).unwrap();
    assert_eq!(
        nested
            .inventory()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["a", "part/a", "part/deeper/a"]
    );
    let mut raw = String::new();
    nested
        .file("part/deeper/a")
        .unwrap()
        .read_to_string(&mut raw)
        .unwrap();
    assert_eq!(raw, "{}");
    for id in ["../a", "part/../a", "/a", "part//a", "part\\a"] {
        assert!(nested.file(id).is_err(), "{id}");
    }
}

#[test]
fn nested_parent_symlinks_are_not_followed() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let outside = temp.path().join("outside");
    fs::create_dir_all(data.join("part")).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(data.join("part/a.json"), "old").unwrap();
    fs::write(outside.join("a.json"), "outside").unwrap();
    let source = Source::open_scoped(&data, true).unwrap();
    source.inventory().unwrap();
    fs::rename(data.join("part"), data.join("old-part")).unwrap();
    symlink(&outside, data.join("part")).unwrap();
    assert!(source.file("part/a").is_err());
    assert!(source.finish().is_err());
}

#[test]
fn combined_inventory_detects_membership_changes_and_refuses_symlinks() {
    for change in ["insert", "delete", "rename", "replace", "symlink"] {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        fs::create_dir(&data).unwrap();
        fs::write(data.join("a.json"), "old").unwrap();
        let source = Source::open(&data).unwrap();
        assert_eq!(source.inventory_ids().unwrap(), ["a"]);
        match change {
            "insert" => fs::write(data.join("b.json"), "new").unwrap(),
            "delete" => fs::remove_file(data.join("a.json")).unwrap(),
            "rename" => fs::rename(data.join("a.json"), data.join("b.json")).unwrap(),
            "replace" => {
                fs::write(data.join("b.json"), "new").unwrap();
                fs::rename(data.join("b.json"), data.join("a.json")).unwrap();
            }
            "symlink" => {
                fs::remove_file(data.join("a.json")).unwrap();
                fs::write(temp.path().join("outside.json"), "outside").unwrap();
                symlink(temp.path().join("outside.json"), data.join("a.json")).unwrap();
                assert!(source.file("a").is_err());
            }
            _ => unreachable!(),
        }
        // Make the directory stamp change explicit; filesystem timestamp
        // granularity may coalesce a create/delete immediately after inventory.
        File::open(&data)
            .unwrap()
            .set_times(
                fs::FileTimes::new()
                    .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(2)),
            )
            .unwrap();
        assert!(source.finish().is_err(), "{change}");
    }
}

#[test]
fn combined_inventory_has_no_prior_stamp_for_in_place_changes() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("a.json"), "old").unwrap();
    let source = Source::open(temp.path()).unwrap();
    assert_eq!(source.inventory_ids().unwrap(), ["a"]);
    let old =
        Stamp::from_metadata(&source.file("a").unwrap().metadata().unwrap(), temp.path()).unwrap();
    fs::write(temp.path().join("a.json"), "new and longer").unwrap();
    let new =
        Stamp::from_metadata(&source.file("a").unwrap().metadata().unwrap(), temp.path()).unwrap();
    assert_ne!(old, new);
    assert!(source.finish().is_ok());
}

#[test]
fn combined_inventory_rejects_recursive_directory_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    fs::create_dir_all(root.join("part")).unwrap();
    fs::write(root.join("part/a.json"), "old").unwrap();
    let source = Source::open_scoped(&root, true).unwrap();
    assert_eq!(source.inventory_ids().unwrap(), ["part/a"]);
    fs::rename(root.join("part"), root.join("old-part")).unwrap();
    fs::create_dir(root.join("part")).unwrap();
    fs::write(root.join("part/a.json"), "new").unwrap();
    assert!(source.finish().is_err());
}
