use super::*;

#[test]
fn missing_file_means_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let c = Config::load(dir.path()).unwrap();
    assert!(c.backup.rules.is_empty() && c.restore.presets.is_empty());
}

#[test]
fn reads_rules_and_presets() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(FILE_NAME),
        "[backup]\nrules = [\"- /go\", \"+ /Library/Pioneer/***\"]\n\n[[restore.presets]]\nname = \"Claude\"\npaths = [\".claude\"]\n",
    )
    .unwrap();
    let c = Config::load(dir.path()).unwrap();
    assert_eq!(c.backup.rules, vec!["- /go", "+ /Library/Pioneer/***"]);
    assert_eq!(c.restore.presets[0].paths, vec![".claude"]);
}

// A typo in the config must stop macup instead of being ignored, otherwise a
// misspelled rule or section would silently change what gets backed up.
#[test]
fn rejects_unknown_keys_and_malformed_rules() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(FILE_NAME), "[backup]\nrule = [\"- /go\"]\n").unwrap();
    assert!(Config::load(dir.path()).is_err());
    std::fs::write(dir.path().join(FILE_NAME), "[backup]\nrules = [\"/go\"]\n").unwrap();
    assert!(Config::load(dir.path()).is_err());
}

#[test]
fn template_parses() {
    let c: Config = toml::from_str(TEMPLATE).unwrap();
    assert!(c.backup.rules.is_empty());
}

#[test]
fn reads_nix_settings() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(FILE_NAME),
        "[nix]\nenabled = false\nconfig = \"~/dotfiles/darwin\"\n",
    )
    .unwrap();
    let c = Config::load(dir.path()).unwrap();
    assert_eq!(c.nix.enabled, Some(false));
    assert_eq!(c.nix.config.as_deref(), Some("~/dotfiles/darwin"));
}

fn install_error(install: &str) -> String {
    Config::parse(&format!("[install]\n{install}\n"))
        .unwrap_err()
        .to_string()
}

// A misspelled list name must stop macup; otherwise what is listed under it
// would silently never be installed. The old list names point to the new ones.
#[test]
fn an_unknown_list_name_is_an_error_with_a_suggestion() {
    assert!(install_error("apps = [\"ghostty\"]").contains("did you mean \"cask\""));
    assert!(install_error("formulae = [\"bat\"]").contains("did you mean \"brew\""));
    assert!(install_error("tool = [\"curl\"]").contains("did you mean \"nix\""));
    assert!(install_error("stuff = []").contains("brew, cask and nix"));
    assert!(install_error("mas = []").contains("aren't supported yet"));
}

// A name under two installers would install it twice and the two would fight
// over PATH; listed twice in one list it is a copy-paste slip.
#[test]
fn a_name_may_appear_only_once() {
    assert!(install_error("brew = [\"htop\"]\nnix = [\"HTOP\"]").contains("pick one installer"));
    assert!(install_error("brew = [\"bat\", \"bat\"]").contains("listed twice"));
}

#[test]
fn third_party_taps_are_refused_until_supported() {
    assert!(install_error("brew = [\"hashicorp/tap/terraform\"]").contains("taps"));
    let c = Config::parse("[install]\nnix = [\"python3Packages.black\"]\n").unwrap();
    assert_eq!(c.install.nix, vec!["python3Packages.black"]);
}

#[test]
fn both_an_old_and_a_new_section_is_an_error() {
    let err = Config::parse("[tools]\nbrew = []\n[install]\nbrew = []\n").unwrap_err();
    assert!(err.to_string().contains("both"), "{err}");
}

#[test]
fn finds_which_list_a_name_is_in_ignoring_case() {
    let c = Config::parse("[install]\ncask = [\"Ghostty\"]\n").unwrap();
    assert_eq!(c.install.installer_of("ghostty"), Some("cask"));
    assert_eq!(c.install.installer_of("bat"), None);
}
