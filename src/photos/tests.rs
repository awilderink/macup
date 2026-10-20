use super::*;
use crate::backup::tests::fixture_env;
use rusqlite::Connection;
use std::fs;

pub const UUID_A: &str = "B51AD57D-03E9-418C-8733-A328B79EC3B6";
pub const UUID_B: &str = "6B84CC4F-718F-432A-80E3-2AE5C1A40FD7";
pub const UUID_C: &str = "003D551F-F4D2-455F-B47E-368930A635C6";

pub fn asset_file(uuid: &str, ext: &str) -> String {
    format!("originals/{}/{uuid}.{ext}", &uuid[..1])
}

pub fn make_library(dir: &std::path::Path, items: usize, originals: &[&str]) {
    let assets: Vec<(String, String, i64)> = (0..items)
        .map(|i| {
            (
                format!("00000000-0000-0000-0000-{i:012}"),
                format!("IMG_{i:04}.JPG"),
                700_000_000,
            )
        })
        .collect();
    make_library_with(dir, &assets, originals);
}

pub fn make_library_with(
    dir: &std::path::Path,
    assets: &[(String, String, i64)],
    originals: &[&str],
) {
    let db = library::db_path(dir);
    fs::create_dir_all(db.parent().unwrap()).unwrap();
    for suffix in ["", "-wal", "-shm"] {
        let _ = fs::remove_file(format!("{}{suffix}", db.display()));
    }
    let conn = Connection::open(&db).unwrap();
    conn.execute_batch(
        "pragma journal_mode=wal;
         create table ZASSET(Z_PK integer primary key, ZUUID text, ZTRASHEDSTATE integer, ZDATECREATED real, ZDIRECTORY text, ZFILENAME text);
         create table ZADDITIONALASSETATTRIBUTES(Z_PK integer primary key, ZASSET integer, ZORIGINALFILENAME text);",
    )
    .unwrap();
    for (i, (uuid, name, created)) in assets.iter().enumerate() {
        let ext = match std::path::Path::new(name)
            .extension()
            .map(|x| x.to_string_lossy().to_lowercase())
            .as_deref()
        {
            Some("jpg") => "jpeg".to_string(),
            Some(other) => other.to_string(),
            None => "bin".to_string(),
        };
        conn.execute(
            "insert into ZASSET values (?1, ?2, 0, ?3, ?4, ?5)",
            rusqlite::params![
                i as i64 + 1,
                uuid,
                (*created - 978_307_200) as f64,
                &uuid[..1],
                format!("{uuid}.{ext}")
            ],
        )
        .unwrap();
        conn.execute(
            "insert into ZADDITIONALASSETATTRIBUTES values (?1, ?1, ?2)",
            rusqlite::params![i as i64 + 1, name],
        )
        .unwrap();
    }
    conn.execute(
        "insert into ZASSET values (100000, 'trashed', 1, 0, null, null)",
        [],
    )
    .unwrap();
    drop(conn);
    for f in originals.iter().map(|s| s.to_string()).chain([
        "resources/derivatives/thumb.jpg".into(),
        "resources/cpl/cloudsync.noindex/upload.bin".into(),
    ]) {
        let p = dir.join(&f);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, &f).unwrap();
    }
}

fn fixture() -> Option<(tempfile::TempDir, Env, PhotoLibrary)> {
    let (dir, e) = fixture_env();
    let src = e.home.join("Pictures/Photos Library.photoslibrary");
    let assets = vec![
        (UUID_A.to_string(), "DSCF3245.JPG".to_string(), 700_000_000),
        (UUID_B.to_string(), "IMG_0737.HEIC".to_string(), 600_000_000),
    ];
    let a = asset_file(UUID_A, "jpeg");
    let b = asset_file(UUID_B, "heic");
    let live = format!("originals/{}/{UUID_B}_3.mov", &UUID_B[..1]);
    make_library_with(&src, &assets, &[&a, &b, &live]);
    let mut libs = photo_libraries(&e);
    assert_eq!(libs.len(), 1);
    Some((dir, e, libs.remove(0)))
}

#[test]
fn backup_copies_library_without_icloud_cache_and_archives_originals() {
    let Some((_dir, e, lib)) = fixture() else {
        return;
    };
    backup_library(&e, false, &lib).unwrap();
    assert!(lib.dst.join(asset_file(UUID_A, "jpeg")).exists());
    assert!(lib.dst.join("resources/derivatives/thumb.jpg").exists());
    assert!(
        !lib.dst.join("resources/cpl").exists(),
        "iCloud upload cache was copied"
    );
    assert_eq!(library::item_count(&lib.dst).unwrap(), 2);
    assert!(
        !lib.staging().exists(),
        "the staging copy was not cleaned up"
    );
    let manifest = archive::load_manifest(&e.archive_dir()).unwrap();
    assert_eq!(manifest.len(), 3, "{manifest:?}");
    assert!(manifest.values().any(|n| n.ends_with("/DSCF3245.JPG")));
    assert!(
        manifest.values().any(|n| n.ends_with("/IMG_0737.mov")),
        "live photo video not named after its photo: {manifest:?}"
    );
}

// With "Optimize Mac Storage" Photos deletes originals from the Mac. The library
// copy follows the Mac, but the archive must keep every original it ever saw.
#[test]
fn originals_removed_on_the_mac_stay_in_the_archive() {
    let Some((_dir, e, lib)) = fixture() else {
        return;
    };
    backup_library(&e, false, &lib).unwrap();
    fs::remove_file(lib.src.join(asset_file(UUID_A, "jpeg"))).unwrap();
    backup_library(&e, false, &lib).unwrap();
    assert!(
        !lib.dst.join(asset_file(UUID_A, "jpeg")).exists(),
        "the library copy should follow the Mac"
    );
    let manifest = archive::load_manifest(&e.archive_dir()).unwrap();
    let archived = &manifest[&format!("{}/{UUID_A}.jpeg", &UUID_A[..1])];
    assert!(
        e.archive_dir().join(archived).exists(),
        "the archived original was lost"
    );
}

#[test]
fn backup_updates_an_existing_copy() {
    let Some((_dir, e, lib)) = fixture() else {
        return;
    };
    backup_library(&e, false, &lib).unwrap();
    let c = asset_file(UUID_C, "png");
    let assets = vec![
        (UUID_A.to_string(), "DSCF3245.JPG".to_string(), 700_000_000),
        (UUID_B.to_string(), "IMG_0737.HEIC".to_string(), 600_000_000),
        (
            UUID_C.to_string(),
            "Screenshot.png".to_string(),
            650_000_000,
        ),
    ];
    make_library_with(&lib.src, &assets, &[&c]);
    fs::create_dir_all(lib.dst.join("resources/cpl")).unwrap();
    backup_library(&e, false, &lib).unwrap();
    assert_eq!(library::item_count(&lib.dst).unwrap(), 3);
    assert!(lib.dst.join(&c).exists(), "new original not copied");
    assert!(
        !lib.dst.join("resources/cpl").exists(),
        "a stale iCloud cache in the backup should be removed"
    );
}

// A run that died half-way leaves a staging copy behind; the next run must
// discard it rather than build on a half-copied library.
#[test]
fn stale_staging_copy_is_discarded() {
    let Some((_dir, e, lib)) = fixture() else {
        return;
    };
    backup_library(&e, false, &lib).unwrap();
    fs::create_dir_all(lib.staging().join("originals/junk")).unwrap();
    fs::write(lib.staging().join("originals/junk/half.jpg"), "x").unwrap();
    backup_library(&e, false, &lib).unwrap();
    assert!(
        !lib.dst.join("originals/junk").exists(),
        "leftovers from the stale staging copy ended up in the backup"
    );
    assert_eq!(library::item_count(&lib.dst).unwrap(), 2);
}

// When a backup fails, the existing backup must stay as it was: the work
// happens on a staging copy that is only swapped in at the end.
#[test]
fn failed_backup_leaves_the_backup_untouched() {
    let Some((_dir, e, lib)) = fixture() else {
        return;
    };
    backup_library(&e, false, &lib).unwrap();
    assert!(
        verify(&lib.dst, 5).is_err(),
        "a copy with fewer items than expected passed verification"
    );
    fs::write(library::db_path(&lib.src), "not a database").unwrap();
    for suffix in ["-wal", "-shm"] {
        let _ = fs::remove_file(format!("{}{suffix}", library::db_path(&lib.src).display()));
    }
    assert!(
        backup_library(&e, false, &lib).is_err(),
        "backing up an unreadable library succeeded"
    );
    assert_eq!(
        library::item_count(&lib.dst).unwrap(),
        2,
        "existing backup changed"
    );
    assert!(
        !lib.staging().exists(),
        "the failed staging copy was left behind"
    );
}

#[test]
fn photo_restore_never_overwrites_an_existing_library() {
    let Some((_dir, e, lib)) = fixture() else {
        return;
    };
    backup_library(&e, false, &lib).unwrap();
    let target = e.home.join("restored/Pictures");
    restore_photos(&e, &target).unwrap();
    let restored = target.join(&lib.name);
    assert_eq!(library::item_count(&restored).unwrap(), 2);
    fs::write(restored.join("marker"), "mine").unwrap();
    restore_photos(&e, &target).unwrap();
    assert!(
        restored.join("marker").exists(),
        "an existing library was overwritten"
    );
}

#[test]
fn originals_missing_from_mac_and_archive_count_as_icloud_only() {
    let Some((_dir, e, lib)) = fixture() else {
        return;
    };
    assert!(
        icloud_only(&e, &lib).unwrap().is_empty(),
        "originals present on the Mac are not iCloud-only"
    );
    backup_library(&e, false, &lib).unwrap();
    assert!(icloud_only(&e, &lib).unwrap().is_empty());
    let c = asset_file(UUID_C, "png");
    let assets = vec![
        (UUID_A.to_string(), "DSCF3245.JPG".to_string(), 700_000_000),
        (UUID_B.to_string(), "IMG_0737.HEIC".to_string(), 600_000_000),
        (
            UUID_C.to_string(),
            "Screenshot.png".to_string(),
            650_000_000,
        ),
    ];
    make_library_with(&lib.src, &assets, &[]);
    fs::remove_file(lib.src.join(&c)).ok();
    let missing = icloud_only(&e, &lib).unwrap();
    assert_eq!(
        missing,
        vec![(
            UUID_C.to_string(),
            c.trim_start_matches("originals/").to_string()
        )]
    );
}

// Files downloaded from iCloud get the same keys the library would use, so a
// later backup that sees the original on the Mac does not archive it twice.
#[test]
fn downloaded_originals_use_library_keys() {
    let rel = format!("B/{UUID_A}.jpeg");
    assert_eq!(
        download_key(&rel, UUID_A, photokit::Kind::Primary, "DSCF3245.JPG"),
        rel
    );
    assert_eq!(
        download_key(&rel, UUID_A, photokit::Kind::PairedVideo, "IMG_1.MOV"),
        format!("B/{UUID_A}_3.mov")
    );
    let dir = tempfile::tempdir().unwrap();
    let mut a = archive::Archive::open(dir.path()).unwrap();
    let file = dir.path().join(".incoming-x");
    fs::write(&file, "photo").unwrap();
    a.add_downloaded(&rel, &file, "DSCF3245.JPG", Some(1_700_000_000))
        .unwrap();
    a.flush().unwrap();
    assert!(
        !file.exists(),
        "the downloaded file should be moved into the archive"
    );
    assert!(archive::load_manifest(dir.path()).unwrap()[&rel].ends_with("/DSCF3245.JPG"));
}
