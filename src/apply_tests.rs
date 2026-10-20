use super::*;

fn install(brew: &[&str], cask: &[&str], rust: bool) -> InstallConfig {
    let list = |l: &[&str]| l.iter().map(|s| s.to_string()).collect();
    InstallConfig {
        brew: list(brew),
        cask: list(cask),
        rust,
    }
}

#[test]
fn writes_one_brewfile_line_per_entry() {
    assert_eq!(
        brewfile(&install(&["bat", "node@22"], &["ghostty"], false)),
        "brew \"bat\"\nbrew \"node@22\"\ncask \"ghostty\"\n"
    );
}

// The lists live on the drive. Without it there is nothing to install from,
// and running anyway would act on an empty list.
#[test]
fn refuses_to_apply_without_the_drive() {
    let dir = tempfile::tempdir().unwrap();
    let e = Env::new(dir.path().join("home"), dir.path().join("Backup"));
    assert!(install_and_apply(&e, false, true, "macup apply").is_err());
    std::fs::create_dir_all(&e.volume).unwrap();
    std::fs::write(e.drive_config(), "[install]\nbrew = [\"bat\"]\n").unwrap();
    assert!(
        install_and_apply(&e, false, true, "macup apply").is_err(),
        "an empty folder is not the mounted drive"
    );
    assert!(!e.cache_dir().exists());
}

// A Mac that only uses macup for backups must not be told to set anything up.
#[test]
fn nothing_listed_counts_as_installed() {
    let dir = tempfile::tempdir().unwrap();
    let e = Env::new(dir.path().join("home"), dir.path().join("Backup"));
    assert!(installed(&e, &install(&[], &[], false)));
    assert!(!installed(&e, &install(&[], &[], true)));
    std::fs::create_dir_all(e.rustup().parent().unwrap()).unwrap();
    std::fs::write(e.rustup(), "").unwrap();
    assert!(installed(&e, &install(&[], &[], true)));
}
