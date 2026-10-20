use super::*;
use crate::guard::{Guard, precomputed, resolve};
use std::fs;

pub fn fixture_env() -> (tempfile::TempDir, Env) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("arvin");
    let volume = dir.path().join("volume");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&volume).unwrap();
    (dir, Env::new(home, volume))
}

fn write(path: &std::path::Path, data: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, data).unwrap();
}

// Plans a backup with the built-in rules against a fake home folder, so a
// rule edit that accidentally starts copying caches, a Photos library or
// macOS's per-machine state fails here.
#[test]
fn embedded_rules_select_the_right_files() {
    let (_dir, e) = fixture_env();
    for f in [
        "Documents/a.txt",
        "Pictures/Photos Library.photoslibrary/database/Photos.sqlite",
        "Pictures/holiday.jpg",
        "Library/Caches/x",
        "Library/Application Support/App/data",
        "Library/Application Support/AddressBook/AddressBook-v22.abcddb",
        "Library/Application Support/FileProvider/x/wharf/tombstone/a",
        "Library/Application Support/CloudDocs/session/db/client.db",
        "code/proj/node_modules/m/index.js",
        ".claude/projects/p/memory/m.md",
        ".claude/file-history/h",
        ".claude/settings.json",
    ] {
        write(&e.home.join(f), "x");
    }
    let spec = backup_spec(&e, &Options::default()).unwrap();
    let mut files: Vec<String> = plan(&spec).files().map(|(n, _, _)| n.to_string()).collect();
    files.sort();
    assert_eq!(
        files,
        vec![
            ".claude/projects/p/memory/m.md",
            ".claude/settings.json",
            "Documents/a.txt",
            "Library/Application Support/AddressBook/AddressBook-v22.abcddb",
            "Library/Application Support/App/data",
            "Pictures/holiday.jpg",
        ]
    );
}

// A freshly installed Mac must not silently overwrite an existing backup with
// default settings: until one real backup has finished, it asks first.
#[test]
fn first_backup_from_this_mac_is_guarded() {
    let (_dir, e) = fixture_env();
    assert!(
        first_backup_guard(&e).is_none(),
        "an empty backup drive should not need confirmation"
    );
    fs::create_dir_all(e.backup_home().join("Documents")).unwrap();
    assert!(
        first_backup_guard(&e).is_some(),
        "first backup over an existing backup was not guarded"
    );
    write(&e.last_backup_file(), "2026-10-01 12:00 · finished\n");
    assert!(
        first_backup_guard(&e).is_none(),
        "a Mac with a previous successful backup should not be asked again"
    );
    assert_eq!(last_backup(&e), "2026-10-01 12:00 · finished");
}

// Without a terminal nobody can answer, so a guard must stop the run unless
// --yes (or a dry run) says to continue.
#[test]
fn guards_stop_non_interactive_runs_unless_yes() {
    let guard = || Some(Guard::new("something is off", "continue?"));
    assert!(resolve(vec![precomputed(guard())], false).is_err());
    assert!(resolve(vec![precomputed(guard())], true).is_ok());
    assert!(resolve(vec![precomputed(None)], false).is_ok());
}

#[test]
fn mirror_guard_counts_what_would_be_deleted() {
    let (_dir, e) = fixture_env();
    write(&e.home.join("kept.txt"), "x");
    for f in ["kept.txt", "gone.txt", "old/a.txt"] {
        write(&e.backup_home().join(f), "x");
    }
    let g = mirror_guard(
        &e,
        &Options {
            mirror: true,
            ..Options::default()
        },
    )
    .unwrap()
    .expect("a guard");
    assert!(g.problem.contains("3 files and folders"), "{}", g.problem);
    assert!(
        e.backup_home().join("gone.txt").exists(),
        "the preview deleted files"
    );
}
