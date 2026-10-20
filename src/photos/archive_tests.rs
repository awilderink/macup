use super::*;
use crate::photos::tests::{UUID_A, UUID_B, asset_file, make_library_with};
use std::fs;

fn day(t: i64) -> String {
    Local
        .timestamp_opt(t, 0)
        .single()
        .unwrap()
        .format("%Y/%Y-%m-%d")
        .to_string()
}

fn asset(name: &str, created: i64) -> Asset {
    Asset {
        original_name: Some(name.into()),
        created: Some(created),
    }
}

#[test]
fn names_primary_files_after_the_original_and_paired_files_after_its_stem() {
    let a = asset("DSCF3245.JPG", 1_700_000_000);
    assert_eq!(
        archive_name(&format!("B/{UUID_A}.jpeg"), Some(&a)),
        format!("{}/DSCF3245.JPG", day(1_700_000_000))
    );
    assert_eq!(
        archive_name(&format!("B/{UUID_A}_3.mov"), Some(&a)),
        format!("{}/DSCF3245.mov", day(1_700_000_000))
    );
    assert_eq!(archive_name("X/odd.bin", None), "unknown/X/odd.bin");
    let undated = Asset {
        original_name: Some("a.jpg".into()),
        created: None,
    };
    assert_eq!(
        archive_name(&format!("B/{UUID_A}.jpeg"), Some(&undated)),
        "undated/a.jpg"
    );
}

#[test]
fn update_is_idempotent_and_never_overwrites_a_different_file() {
    let dir = tempfile::tempdir().unwrap();
    let lib = dir.path().join("lib.photoslibrary");
    let archive = dir.path().join("archive");
    let a = asset_file(UUID_A, "jpeg");
    let b = asset_file(UUID_B, "jpeg");
    let assets = vec![
        (
            UUID_A.to_string(),
            "IMG_0001.JPG".to_string(),
            1_700_000_000,
        ),
        (
            UUID_B.to_string(),
            "IMG_0001.JPG".to_string(),
            1_700_000_000,
        ),
    ];
    make_library_with(&lib, &assets, &[&a]);
    fs::create_dir_all(lib.join(&b).parent().unwrap()).unwrap();
    fs::write(lib.join(&b), "a different photo with the same name and day").unwrap();
    let first = update(&archive, &lib).unwrap();
    assert_eq!(first.added, 2);
    let names: Vec<String> = load_manifest(&archive).unwrap().into_values().collect();
    assert!(
        names.iter().any(|n| n.ends_with("/IMG_0001.JPG")),
        "{names:?}"
    );
    let suffixed = |u: &str| format!("/IMG_0001 ({}).JPG", &u[..8]);
    assert!(
        names
            .iter()
            .any(|n| n.ends_with(&suffixed(UUID_A)) || n.ends_with(&suffixed(UUID_B))),
        "{names:?}"
    );
    let second = update(&archive, &lib).unwrap();
    assert_eq!(
        second,
        Stats {
            added: 0,
            bytes: 0,
            total: 2
        }
    );
}

// A crash between cloning a file and writing the manifest leaves an unclaimed
// file in the archive; the next run must adopt it instead of failing.
#[test]
fn adopts_a_file_left_by_an_interrupted_run() {
    let dir = tempfile::tempdir().unwrap();
    let lib = dir.path().join("lib.photoslibrary");
    let archive = dir.path().join("archive");
    let a = asset_file(UUID_A, "jpeg");
    make_library_with(
        &lib,
        &[(
            UUID_A.to_string(),
            "IMG_0001.JPG".to_string(),
            1_700_000_000,
        )],
        &[&a],
    );
    let leftover = archive.join(format!("{}/IMG_0001.JPG", day(1_700_000_000)));
    fs::create_dir_all(leftover.parent().unwrap()).unwrap();
    fs::copy(lib.join(&a), &leftover).unwrap();
    let stats = update(&archive, &lib).unwrap();
    assert_eq!((stats.added, stats.total), (0, 1));
}

#[test]
fn archived_files_carry_the_date_the_photo_was_taken() {
    let dir = tempfile::tempdir().unwrap();
    let lib = dir.path().join("lib.photoslibrary");
    let archive = dir.path().join("archive");
    let a = asset_file(UUID_A, "jpeg");
    make_library_with(
        &lib,
        &[(
            UUID_A.to_string(),
            "IMG_0001.JPG".to_string(),
            1_500_000_000,
        )],
        &[&a],
    );
    update(&archive, &lib).unwrap();
    let file = archive.join(format!("{}/IMG_0001.JPG", day(1_500_000_000)));
    let modified = fs::metadata(file).unwrap().modified().unwrap();
    assert_eq!(
        modified
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        1_500_000_000
    );
}
