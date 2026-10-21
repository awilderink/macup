use super::*;

fn backup(skip: &[&str], include: &[&str]) -> Filter {
    let mut config = Config::default();
    config.backup.skip = skip.iter().map(|s| s.to_string()).collect();
    config.backup.include = include.iter().map(|s| s.to_string()).collect();
    Filter::backup(&config, &[]).unwrap()
}

#[test]
fn builtin_rules_parse() {
    builtin().unwrap();
}

// ~/Library holds per-machine state that must not travel to another Mac; only
// Application Support is backed up, minus caches.
#[test]
fn library_is_backed_up_only_for_application_support() {
    let f = backup(&[], &[]);
    assert!(f.copies("Library", true));
    assert!(f.copies("Library/Application Support/App/data", false));
    assert!(!f.copies("Library/Preferences", true));
    assert!(!f.copies("Library/foo.plist", false));
    assert!(!f.copies("Library/Application Support/Caches", true));
}

#[test]
fn claude_is_backed_up_only_for_its_settings_and_memory() {
    let f = backup(&[], &[]);
    assert!(f.copies(".claude/projects", true));
    assert!(f.copies(".claude/projects/p", true));
    assert!(f.copies(".claude/projects/p/memory/m.md", false));
    assert!(!f.copies(".claude/projects/p/log.jsonl", false));
    assert!(!f.copies(".claude/file-history", true));
    assert!(f.copies(".claude/settings.json", false));
}

// include brings back what macup leaves out by default, but never what a skip
// rules out: an included app folder still loses its caches.
#[test]
fn include_brings_back_what_is_left_out_but_skips_still_win() {
    let f = backup(
        &["/Library/Pioneer/Logs"],
        &["/Library/Pioneer", "/.npm/config"],
    );
    assert!(f.copies("Library/Pioneer/rekordbox/db", false));
    assert!(!f.copies("Library/Pioneer/Cache", true));
    assert!(!f.copies("Library/Pioneer/Logs", true));
    assert!(f.copies(".npm", true));
    assert!(f.copies(".npm/config/x", false));
    assert!(!f.copies(".npm/_cacache", true));
}

#[test]
fn leveldb_logs_are_kept_although_logs_are_skipped() {
    let f = backup(&[], &[]);
    assert!(!f.copies("code/app/debug.log", false));
    assert!(f.copies(
        "Library/Application Support/App/Local Storage/leveldb/000003.log",
        false
    ));
}

#[test]
fn include_must_point_at_something_left_out() {
    assert!(check_include("/Library/Pioneer").is_ok());
    assert!(check_include("/.rustup/settings.toml").is_ok());
    assert!(check_include("/Documents").is_err());
    assert!(check_include("Pioneer").is_err());
}
