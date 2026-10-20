use super::*;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;

fn write(path: &Path, data: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, data).unwrap();
}

fn setup() -> (tempfile::TempDir, Spec) {
    let dir = tempfile::tempdir().unwrap();
    let spec = Spec {
        src: dir.path().join("src"),
        dst: dir.path().join("dst"),
        ..Spec::default()
    };
    fs::create_dir_all(&spec.src).unwrap();
    (dir, spec)
}

fn run(spec: &Spec) -> Outcome {
    let log = spec.dst.with_extension("log");
    Job {
        spec: spec.clone(),
        log_path: log,
        strict: false,
    }
    .run(|_| {}, |_| {})
    .unwrap()
}

fn set_xattr(path: &Path, name: &str, value: &[u8]) {
    let (p, n) = (cstr(path).unwrap(), CString::new(name).unwrap());
    let rc = unsafe {
        libc::setxattr(
            p.as_ptr(),
            n.as_ptr(),
            value.as_ptr().cast(),
            value.len(),
            0,
            0,
        )
    };
    assert_eq!(rc, 0, "{}", std::io::Error::last_os_error());
}

fn get_xattr(path: &Path, name: &str) -> Option<Vec<u8>> {
    let (p, n) = (cstr(path).unwrap(), CString::new(name).unwrap());
    let mut buf = vec![0u8; 256];
    let len = unsafe {
        libc::getxattr(
            p.as_ptr(),
            n.as_ptr(),
            buf.as_mut_ptr().cast(),
            buf.len(),
            0,
            0,
        )
    };
    (len >= 0).then(|| buf[..len as usize].to_vec())
}

// A backup is only useful if it carries what the file was: its contents, when
// it was made and changed, who may read it and its extended attributes.
#[test]
fn copies_contents_times_permissions_and_extended_attributes() {
    let (_dir, spec) = setup();
    let a = spec.src.join("docs/a.txt");
    write(&a, "hello");
    fs::set_permissions(&a, fs::Permissions::from_mode(0o640)).unwrap();
    set_xattr(&a, "com.example.tag", b"red");
    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000);
    File::options()
        .write(true)
        .open(&a)
        .unwrap()
        .set_times(FileTimes::new().set_modified(old).set_created(old))
        .unwrap();
    assert!(run(&spec).finished());
    let b = spec.dst.join("docs/a.txt");
    let meta = fs::metadata(&b).unwrap();
    assert_eq!(fs::read_to_string(&b).unwrap(), "hello");
    assert_eq!(meta.permissions().mode() & 0o777, 0o640);
    assert_eq!(meta.modified().unwrap(), old);
    assert_eq!(meta.created().unwrap(), old);
    assert_eq!(
        get_xattr(&b, "com.example.tag").as_deref(),
        Some(&b"red"[..])
    );
}

// The second run must find nothing to copy, otherwise every backup would copy
// the whole home folder again.
#[test]
fn an_unchanged_tree_needs_no_copying() {
    let (_dir, spec) = setup();
    write(&spec.src.join("a/b.txt"), "x");
    symlink("b.txt", spec.src.join("a/link")).unwrap();
    run(&spec);
    let again = plan(&spec);
    assert_eq!(again.files().count(), 0, "{:?}", again.actions);
    assert!(
        !again
            .actions
            .iter()
            .any(|a| matches!(a, Action::Link { .. }))
    );
    write(&spec.src.join("a/b.txt"), "changed");
    assert_eq!(plan(&spec).files().count(), 1);
}

// A symlink is backed up as a link. Following it would copy whatever it points
// at, possibly far outside the home folder.
#[test]
fn symlinks_are_copied_as_links_and_never_followed() {
    let (dir, spec) = setup();
    let outside = dir.path().join("outside");
    write(&outside.join("secret.txt"), "s");
    symlink(&outside, spec.src.join("elsewhere")).unwrap();
    run(&spec);
    let link = spec.dst.join("elsewhere");
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_link(&link).unwrap(), outside);
}

// Between two runs a file can become a folder and back, or a folder can become
// a link. The copy must follow without writing through a link it finds in the
// destination.
#[test]
fn follows_type_changes_without_writing_through_links() {
    let (dir, spec) = setup();
    write(&spec.src.join("x/inner.txt"), "dir");
    write(&spec.src.join("y"), "file");
    run(&spec);
    fs::remove_dir_all(spec.src.join("x")).unwrap();
    write(&spec.src.join("x"), "now a file");
    fs::remove_file(spec.src.join("y")).unwrap();
    write(&spec.src.join("y/inner.txt"), "now a dir");
    assert!(run(&spec).finished());
    assert_eq!(
        fs::read_to_string(spec.dst.join("x")).unwrap(),
        "now a file"
    );
    assert_eq!(
        fs::read_to_string(spec.dst.join("y/inner.txt")).unwrap(),
        "now a dir"
    );

    let outside = dir.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::remove_dir_all(spec.dst.join("y")).unwrap();
    symlink(&outside, spec.dst.join("y")).unwrap();
    assert!(run(&spec).finished());
    assert!(fs::symlink_metadata(spec.dst.join("y")).unwrap().is_dir());
    assert!(
        fs::read_dir(&outside).unwrap().next().is_none(),
        "wrote through a link in the destination"
    );
}

// Folders like ~/.cargo/registry sources are read-only. Their copy is too, and
// the next run must still be able to update what is inside.
#[test]
fn read_only_folders_can_be_updated_on_the_next_run() {
    let (_dir, spec) = setup();
    let ro = spec.src.join("ro");
    write(&ro.join("a.txt"), "1");
    fs::set_permissions(&ro, fs::Permissions::from_mode(0o555)).unwrap();
    assert!(run(&spec).finished());
    assert_eq!(
        fs::metadata(spec.dst.join("ro"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o555
    );
    fs::set_permissions(&ro, fs::Permissions::from_mode(0o755)).unwrap();
    write(&ro.join("a.txt"), "2");
    write(&ro.join("b.txt"), "new");
    fs::set_permissions(&ro, fs::Permissions::from_mode(0o555)).unwrap();
    assert!(run(&spec).finished());
    assert_eq!(fs::read_to_string(spec.dst.join("ro/a.txt")).unwrap(), "2");
    assert!(spec.dst.join("ro/b.txt").exists());
    fs::set_permissions(&ro, fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(spec.dst.join("ro"), fs::Permissions::from_mode(0o755)).unwrap();
}

// Mirror mode removes what is gone from the Mac, but what the rules leave out
// is not "gone": a skipped folder in the backup must survive.
#[test]
fn mirror_deletes_what_is_gone_but_keeps_what_the_rules_leave_out() {
    let (_dir, mut spec) = setup();
    write(&spec.src.join("kept.txt"), "x");
    for f in ["kept.txt", "gone.txt", "old/a.txt", "skipped/s.txt"] {
        write(&spec.dst.join(f), "x");
    }
    spec.filter = Filter::skipping(&["/skipped".to_string()]).unwrap();
    spec.mirror = true;
    let p = plan(&spec);
    let mut gone: Vec<&str> = p.deletions().collect();
    gone.sort();
    assert_eq!(gone, vec!["gone.txt", "old", "old/a.txt"]);
    assert!(run(&spec).finished());
    assert!(spec.dst.join("kept.txt").exists());
    assert!(!spec.dst.join("old").exists());
    assert!(spec.dst.join("skipped/s.txt").exists());
}

// Hidden paths, like Photos' iCloud upload cache, are neither copied nor kept
// in a mirrored copy.
#[test]
fn hidden_paths_are_not_copied_and_removed_from_a_mirror() {
    let (_dir, mut spec) = setup();
    write(&spec.src.join("resources/cpl/upload.bin"), "x");
    write(&spec.src.join("resources/derivatives/t.jpg"), "x");
    write(&spec.dst.join("resources/cpl/old.bin"), "x");
    spec.hide = vec![Pattern::parse("/resources/cpl/").unwrap()];
    spec.mirror = true;
    assert!(run(&spec).finished());
    assert!(spec.dst.join("resources/derivatives/t.jpg").exists());
    assert!(!spec.dst.join("resources/cpl").exists());
}

// An interrupted run leaves a temporary file next to its target. It must never
// be counted or copied, and the next run cleans it up.
#[test]
fn temporary_files_are_ignored_and_cleaned_up() {
    let (_dir, spec) = setup();
    write(&spec.src.join("a.txt"), "x");
    write(&spec.src.join(format!("{TEMP_PREFIX}1-b.txt")), "partial");
    write(&spec.dst.join(format!("{TEMP_PREFIX}9-a.txt")), "partial");
    let p = plan(&spec);
    assert_eq!(
        p.files().map(|(n, _, _)| n).collect::<Vec<_>>(),
        vec!["a.txt"]
    );
    assert!(run(&spec).finished());
    let names: Vec<String> = fs::read_dir(&spec.dst)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["a.txt"]);
}

// One unreadable file must not stop a backup of everything else; it is
// reported, and only a strict copy (Photos) treats it as a failure.
#[test]
fn an_unreadable_file_is_reported_and_the_rest_is_copied() {
    let (_dir, spec) = setup();
    write(&spec.src.join("a.txt"), "a");
    write(&spec.src.join("locked.txt"), "l");
    write(&spec.src.join("z.txt"), "z");
    fs::set_permissions(
        spec.src.join("locked.txt"),
        fs::Permissions::from_mode(0o000),
    )
    .unwrap();
    let res = run(&spec);
    fs::set_permissions(
        spec.src.join("locked.txt"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert_eq!(res.status, Status::Partial(1));
    assert!(res.ok());
    assert!(spec.dst.join("a.txt").exists() && spec.dst.join("z.txt").exists());
    assert!(
        !Outcome {
            strict: true,
            ..res
        }
        .ok()
    );
}

// A restore of a few folders copies just those, creating the folders above
// them without changing ones that already exist on the Mac.
#[test]
fn a_selection_copies_only_the_chosen_paths() {
    let (_dir, mut spec) = setup();
    write(&spec.src.join("Documents/taxes/2025.pdf"), "t");
    write(&spec.src.join("Documents/other.txt"), "o");
    write(&spec.src.join("Music/song.mp3"), "m");
    fs::create_dir_all(spec.dst.join("Documents")).unwrap();
    fs::set_permissions(
        spec.dst.join("Documents"),
        fs::Permissions::from_mode(0o750),
    )
    .unwrap();
    spec.selection = Some(vec!["Documents/taxes".to_string()]);
    assert!(run(&spec).finished());
    assert!(spec.dst.join("Documents/taxes/2025.pdf").exists());
    assert!(!spec.dst.join("Documents/other.txt").exists());
    assert!(!spec.dst.join("Music").exists());
    assert_eq!(
        fs::metadata(spec.dst.join("Documents"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o750
    );
}

// rsync never deletes what the rules leave out, also not inside a folder that
// is gone from the Mac; that folder then stays, holding only those files.
#[test]
fn mirror_keeps_skipped_files_inside_a_folder_that_is_gone() {
    let (_dir, mut spec) = setup();
    for f in ["gone/a.txt", "gone/.DS_Store", "gone/sub/b.txt"] {
        write(&spec.dst.join(f), "x");
    }
    spec.filter = Filter::skipping(&[".DS_Store".to_string()]).unwrap();
    spec.mirror = true;
    let mut gone: Vec<String> = plan(&spec).deletions().map(str::to_string).collect();
    gone.sort();
    assert_eq!(gone, vec!["gone/a.txt", "gone/sub", "gone/sub/b.txt"]);
    assert!(run(&spec).finished());
    assert!(spec.dst.join("gone/.DS_Store").exists());
    assert!(!spec.dst.join("gone/a.txt").exists() && !spec.dst.join("gone/sub").exists());
}

// An extended attribute that cannot be set must not cost the file itself:
// rsync keeps the file and reports the attribute, and so must the copier.
#[test]
fn a_failing_extended_attribute_keeps_the_file() {
    let (_dir, spec) = setup();
    let a = spec.src.join("a.txt");
    write(&a, "data");
    let notes = copy_xattrs(&spec.src.join("missing"), &a);
    assert_eq!(notes.len(), 1, "{notes:?}");
    set_xattr(&a, "com.example.tag", b"x");
    let unwritable = spec.src.join("nowhere/b.txt");
    let notes = copy_xattrs(&a, &unwritable);
    assert!(
        notes.iter().any(|n| n.contains("com.example.tag")),
        "{notes:?}"
    );
}
