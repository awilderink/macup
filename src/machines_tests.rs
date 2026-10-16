use super::*;
use crate::backup::tests::fixture_env;
use std::fs;

// Drives from before per-Mac folders hold home/ and photos/ at the root; they
// must move under macs/<this Mac>/ in one go without copying anything.
#[test]
fn migrates_the_single_mac_layout() {
    let (_dir, e) = fixture_env();
    fs::create_dir_all(e.volume.join("home/Documents")).unwrap();
    fs::write(e.volume.join("home/Documents/a.txt"), "a").unwrap();
    fs::create_dir_all(e.volume.join("photos/originals")).unwrap();
    migrate_legacy(&e).unwrap();
    assert!(!has_legacy_layout(&e));
    assert!(e.backup_home().join("Documents/a.txt").exists());
    assert!(e.photos_dir().join("originals").exists());
    assert!(!e.lock_dir().exists(), "the lock was not released");
}

#[test]
fn refuses_to_merge_old_and_new_layouts() {
    let (_dir, e) = fixture_env();
    fs::create_dir_all(e.volume.join("home")).unwrap();
    fs::create_dir_all(e.backup_home()).unwrap();
    assert!(migrate_legacy(&e).is_err());
    assert!(e.volume.join("home").exists());
}

// After a reinstall the Mac's name can differ from the one that made the
// backup; with a single Mac on the drive that backup is still the right source.
#[test]
fn picks_the_only_backed_up_mac_when_this_one_has_none() {
    let (_dir, e) = fixture_env();
    fs::create_dir_all(e.for_machine("Old-Mac").backup_home()).unwrap();
    assert_eq!(source(&e, None).unwrap().machine, "Old-Mac");
    fs::create_dir_all(e.backup_home()).unwrap();
    assert_eq!(source(&e, None).unwrap().machine, e.machine);
    assert_eq!(source(&e, Some("Old-Mac")).unwrap().machine, "Old-Mac");
    assert!(source(&e, Some("Nope")).is_err());
}

#[test]
fn several_other_macs_need_a_choice() {
    let (_dir, e) = fixture_env();
    for m in ["A", "B"] {
        fs::create_dir_all(e.for_machine(m).backup_home()).unwrap();
    }
    assert!(source(&e, None).is_err());
}

// A dry run promises to touch nothing, so it must not move an old-layout
// drive; it explains instead, and a real run does the move.
#[test]
fn dry_runs_never_migrate() {
    let (_dir, e) = fixture_env();
    fs::create_dir_all(e.volume.join("home/Documents")).unwrap();
    assert!(prepare(&e, false).is_err());
    assert!(
        e.volume.join("home/Documents").exists(),
        "a dry run moved the backup"
    );
}
