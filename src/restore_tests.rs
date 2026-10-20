use super::*;
use crate::backup::tests::fixture_env;
use crate::photos::tests::make_library;
use std::fs;

#[test]
fn clean_rel_rejects_escapes_and_normalises() {
    assert_eq!(
        clean_rel("/Documents//taxes/"),
        Some("Documents/taxes".into())
    );
    assert_eq!(clean_rel("./.ssh"), Some(".ssh".into()));
    assert_eq!(clean_rel("../etc"), None);
    assert_eq!(clean_rel("/"), None);
}

#[test]
fn restore_modes_handle_newer_local_files() {
    let (_dir, e) = fixture_env();
    fs::create_dir_all(e.backup_home().join("Documents")).unwrap();
    fs::write(e.backup_home().join("Documents/a.txt"), "backup").unwrap();
    fs::write(e.backup_home().join("Documents/b.txt"), "b").unwrap();
    let target = e.home.clone();
    fs::create_dir_all(target.join("Documents")).unwrap();
    fs::write(target.join("Documents/a.txt"), "local and newer").unwrap();
    let future = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
    fs::File::options()
        .write(true)
        .open(target.join("Documents/a.txt"))
        .unwrap()
        .set_modified(future)
        .unwrap();
    let docs = vec!["Documents".to_string()];
    restore_paths(&e, &target, &docs, Some(Mode::KeepNewer), false, true).unwrap();
    assert_eq!(
        fs::read_to_string(target.join("Documents/a.txt")).unwrap(),
        "local and newer"
    );
    assert_eq!(
        fs::read_to_string(target.join("Documents/b.txt")).unwrap(),
        "b"
    );
    restore_paths(&e, &target, &docs, Some(Mode::Overwrite), false, true).unwrap();
    assert_eq!(
        fs::read_to_string(target.join("Documents/a.txt")).unwrap(),
        "backup"
    );
}

// Restoring a folder that contains a Photos library must never copy the
// backed-up library into one that already exists on this Mac: two libraries'
// databases and originals would be mixed and the current one damaged.
#[test]
fn restore_skips_an_existing_photos_library() {
    let (_dir, e) = fixture_env();
    let backed_up = e
        .backup_home()
        .join("Pictures/Photos Library.photoslibrary");
    make_library(&backed_up, 5, &["originals/O/old.jpg"]);
    fs::write(e.backup_home().join("Pictures/holiday.jpg"), "x").unwrap();
    let current = e.home.join("Pictures/Photos Library.photoslibrary");
    make_library(&current, 2, &["originals/A/a.jpg"]);
    restore_paths(
        &e,
        &e.home,
        &["Pictures".to_string()],
        Some(Mode::KeepNewer),
        false,
        true,
    )
    .unwrap();
    assert!(
        !current.join("originals/O/old.jpg").exists(),
        "the backed-up library was copied into the existing one"
    );
    assert_eq!(crate::photos::library::item_count(&current).unwrap(), 2);
    assert!(
        e.home.join("Pictures/holiday.jpg").exists(),
        "the rest of Pictures was not restored"
    );
}

#[test]
fn ssh_permissions_are_fixed_after_restore() {
    let (_dir, e) = fixture_env();
    fs::create_dir_all(e.backup_home().join(".ssh")).unwrap();
    fs::write(e.backup_home().join(".ssh/id_ed25519"), "key").unwrap();
    fs::write(e.backup_home().join(".ssh/id_ed25519.pub"), "pub").unwrap();
    fs::set_permissions(
        e.backup_home().join(".ssh/id_ed25519"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    restore_paths(
        &e,
        &e.home,
        &[".ssh".to_string()],
        Some(Mode::KeepNewer),
        false,
        true,
    )
    .unwrap();
    let mode = |p: &str| fs::metadata(e.home.join(p)).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(".ssh"), 0o700);
    assert_eq!(mode(".ssh/id_ed25519"), 0o600);
    assert_eq!(mode(".ssh/id_ed25519.pub"), 0o644);
}
