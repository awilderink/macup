use super::*;

#[test]
fn missing_file_means_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let c = Config::load(dir.path()).unwrap();
    assert!(c.backup.skip.is_empty() && c.restore.presets.is_empty());
}

#[test]
fn reads_skip_keep_and_presets() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(FILE_NAME),
        "[backup]\nskip = [\"/go\"]\nkeep = [\"/Library/Pioneer\"]\n\n[[restore.presets]]\nname = \"Claude\"\npaths = [\".claude\"]\n",
    )
    .unwrap();
    let c = Config::load(dir.path()).unwrap();
    assert_eq!(c.backup.skip, vec!["/go"]);
    assert_eq!(c.backup.keep, vec!["/Library/Pioneer"]);
    assert_eq!(c.restore.presets[0].paths, vec![".claude"]);
}

// Rules written for rsync keep working: each becomes a skip or a keep that
// selects the same files.
#[test]
fn converts_old_rsync_rules() {
    let rules: Vec<String> = [
        "- ComfyUI",
        "- /Library/Application Support/Native Instruments",
        "- /code/**/target/",
        "+ /Library/Pioneer/",
        "+ /Library/Pioneer/***",
        "- /go",
        "- /.fig*",
        "- /.claude/projects/***",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let (skip, keep) = convert_rules(&rules).unwrap();
    assert_eq!(
        skip,
        vec![
            "ComfyUI",
            "/Library/Application Support/Native Instruments",
            "/code/**/target/",
            "/go",
            "/.fig*",
            "/.claude/projects",
        ]
    );
    assert_eq!(keep, vec!["/Library/Pioneer"]);
    let c = Config::parse("[backup]\nrules = [\"- /go\", \"+ /Library/Pioneer/***\"]\n").unwrap();
    assert!(c.old_layout);
    assert_eq!(
        (c.backup.skip, c.backup.keep),
        (
            vec!["/go".to_string()],
            vec!["/Library/Pioneer".to_string()]
        )
    );
}

// With rsync a skip after a keep could not reach inside it; with skip and keep,
// skips always win. Converting such a pair would change what is backed up, so
// it stops and says so.
#[test]
fn refuses_rules_whose_order_matters() {
    let rules = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert!(convert_rules(&rules(&["+ /Library/Pioneer/***", "- /Library/Pioneer/x"])).is_err());
    assert!(convert_rules(&rules(&["+ /Library/Pioneer/***", "- *.bak"])).is_err());
    assert!(convert_rules(&rules(&["- *.bak", "+ /Library/Pioneer/***"])).is_ok());
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
    std::fs::write(dir.path().join(FILE_NAME), "[backup]\nskip = [\"- /go\"]\n").unwrap();
    assert!(Config::load(dir.path()).is_err());
    std::fs::write(
        dir.path().join(FILE_NAME),
        "[backup]\nkeep = [\"/Documents\"]\n",
    )
    .unwrap();
    assert!(Config::load(dir.path()).is_err());
}

#[test]
fn template_parses() {
    let c = Config::parse(TEMPLATE).unwrap();
    assert!(c.backup.skip.is_empty() && c.backup.keep.is_empty());
}

// A drive written while macup still used Nix must keep loading: the nix list
// maps onto what replaced it, and the [nix] section no longer means anything.
#[test]
fn reads_a_drive_from_the_nix_days() {
    let c = Config::parse(
        "[install]\nbrew = [\"bat\"]\nnix = [\"curl\", \"git\", \"rustup\"]\n\n[nix]\nenabled = true\n",
    )
    .unwrap();
    assert!(c.old_layout);
    assert!(c.install.rust);
    assert_eq!(c.install.brew, vec!["bat"]);
    let c = Config::parse("[install]\nnix = [\"git\"]\n").unwrap();
    assert!(!c.install.rust);
}

// Anything else on the old nix list has no new home macup could guess, so it
// must stop with the fix instead of silently dropping it.
#[test]
fn an_unknown_nix_entry_says_where_it_goes() {
    let err = Config::parse("[install]\nnix = [\"htop\"]\n").unwrap_err();
    assert!(err.to_string().contains("macup add --brew htop"), "{err}");
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
    assert!(install_error("rustup = true").contains("did you mean \"rust\""));
    assert!(install_error("stuff = []").contains("brew, cask and rust"));
    assert!(install_error("mas = []").contains("aren't supported yet"));
}

// A name under two installers would install it twice and the two would fight
// over PATH; listed twice in one list it is a copy-paste slip.
#[test]
fn a_name_may_appear_only_once() {
    assert!(
        install_error("brew = [\"docker\"]\ncask = [\"Docker\"]").contains("pick one installer")
    );
    assert!(install_error("brew = [\"bat\", \"bat\"]").contains("listed twice"));
}

#[test]
fn third_party_taps_are_refused_until_supported() {
    assert!(install_error("brew = [\"hashicorp/tap/terraform\"]").contains("taps"));
}

// Names end up in a Brewfile, which Homebrew evaluates as Ruby; a quote or
// interpolation in a name would break it or run code.
#[test]
fn only_homebrew_name_characters_are_accepted() {
    assert!(install_error("brew = [\"bat\\\" ; system \\\"x\"]").contains("not a Homebrew name"));
    assert!(install_error("brew = [\"#{x}\"]").contains("not a Homebrew name"));
    assert!(install_error("brew = [\"\"]").contains("not a Homebrew name"));
    let c =
        Config::parse("[install]\nbrew = [\"node@22\", \"c++-lib\", \"python3.12\"]\n").unwrap();
    assert_eq!(c.install.brew.len(), 3);
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
