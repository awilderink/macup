use super::*;

fn env(dir: &Path) -> Env {
    Env::new(dir.join("home"), dir.join("Backup"))
}

fn install(brew: &[&str], cask: &[&str], nix: &[&str]) -> InstallConfig {
    let list = |l: &[&str]| l.iter().map(|s| s.to_string()).collect();
    InstallConfig {
        brew: list(brew),
        cask: list(cask),
        nix: list(nix),
    }
}

// The lists live on the drive. Building without it would apply empty lists
// and uninstall every tool and app, so it must refuse before writing anything.
#[test]
fn refuses_to_prepare_without_the_drive() {
    let dir = tempfile::tempdir().unwrap();
    let e = env(dir.path());
    assert!(prepare(&e).is_err());
    std::fs::create_dir_all(&e.volume).unwrap();
    std::fs::write(e.drive_config(), "[install]\nnix = [\"curl\"]\n").unwrap();
    assert!(
        prepare(&e).is_err(),
        "an empty folder is not the mounted drive"
    );
    assert!(!e.flake_dir().exists());
}

#[test]
fn writes_the_lists_next_to_the_flake() {
    let dir = tempfile::tempdir().unwrap();
    let flake = dir.path().join("nix");
    write_flake(
        &flake,
        "arvin",
        &install(&["bat"], &["ghostty"], &["curl"]),
        None,
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(flake.join("flake.nix")).unwrap(),
        FLAKE
    );
    let mac: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(flake.join("mac.json")).unwrap()).unwrap();
    assert_eq!(
        mac,
        serde_json::json!({"user": "arvin", "brew": ["bat"], "cask": ["ghostty"], "nix": ["curl"]})
    );
    assert!(!flake.join("flake.lock").exists());
}

// The pinned versions on the drive win over whatever an earlier run left in
// the cache, so every Mac builds the same versions; without a lock on the
// drive the cached one is kept.
#[test]
fn takes_the_lock_from_the_drive_when_there_is_one() {
    let dir = tempfile::tempdir().unwrap();
    let flake = dir.path().join("nix");
    let empty = install(&[], &[], &[]);
    write_flake(&flake, "arvin", &empty, Some(b"drive")).unwrap();
    assert_eq!(std::fs::read(flake.join("flake.lock")).unwrap(), b"drive");
    write_flake(&flake, "arvin", &empty, None).unwrap();
    assert_eq!(std::fs::read(flake.join("flake.lock")).unwrap(), b"drive");
}

#[test]
fn an_own_flake_is_used_as_it_is() {
    let dir = tempfile::tempdir().unwrap();
    let own = dir.path().join("darwin");
    let mut config = Config::default();
    config.nix.config = Some(own.display().to_string());
    let e = env(dir.path()).with_config(config);
    assert!(prepare(&e).is_err(), "no flake.nix there yet");
    std::fs::create_dir_all(&own).unwrap();
    std::fs::write(own.join("flake.nix"), "{}").unwrap();
    assert_eq!(prepare(&e).unwrap(), own);
    assert!(!e.flake_dir().exists());
}
