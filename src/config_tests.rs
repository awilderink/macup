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
