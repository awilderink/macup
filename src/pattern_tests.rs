use super::*;

fn p(text: &str) -> Pattern {
    Pattern::parse(text).unwrap()
}

// These follow rsync's filter matching, so rules that worked with rsync keep
// selecting the same files.
#[test]
fn a_name_without_a_slash_matches_at_any_depth() {
    assert!(p("node_modules").matches("code/app/node_modules", true));
    assert!(p("*.log").matches("a/b/x.log", false));
    assert!(!p("*.log").matches("a/x.log/b", false));
    assert!(p(".zcompdump*").matches(".zcompdump-mac-5.9", false));
}

#[test]
fn a_leading_slash_anchors_at_the_home_folder() {
    assert!(p("/go").matches("go", true));
    assert!(!p("/go").matches("code/go", true));
    assert!(p("/.fig*").matches(".fig", true));
    assert!(
        p("/Library/Application Support/Caches")
            .matches("Library/Application Support/Caches", true)
    );
}

#[test]
fn a_trailing_slash_means_folders_only() {
    assert!(p("cache/").matches("x/cache", true));
    assert!(!p("cache/").matches("x/cache", false));
    assert!(p("/code/**/target/").matches("code/a/b/target", true));
    assert!(!p("/code/**/target/").matches("code/a/target", false));
}

#[test]
fn a_slash_inside_matches_the_end_of_the_path() {
    let leveldb = p("*leveldb/*.log");
    assert!(leveldb.matches("Library/x/Local Storage/leveldb/000003.log", false));
    assert!(!leveldb.matches("Library/x/000003.log", false));
}

#[test]
fn stars_stay_within_a_name_and_double_stars_cross_folders() {
    assert!(glob_match("a*c", "abbc"));
    assert!(!glob_match("a*c", "ab/c"));
    assert!(glob_match("a/**/c", "a/b/b/c"));
    assert!(glob_match("?b", "ab") && !glob_match("?b", "/b"));
    assert!(glob_match("[a-c]x", "bx") && !glob_match("[!a-c]x", "bx"));
}

#[test]
fn covers_what_is_inside_a_match() {
    assert!(p("/.npm").covers(".npm/_cacache/x", false));
    assert!(!p("/.npm").covers(".npmrc", false));
}

#[test]
fn knows_which_folders_lead_to_a_match() {
    let memory = p("/.claude/projects/*/memory");
    assert!(memory.leads_into(".claude"));
    assert!(memory.leads_into(".claude/projects/p"));
    assert!(!memory.leads_into(".claude/projects/p/memory"));
    assert!(!memory.leads_into(".claude/file-history"));
}

#[test]
fn old_rsync_syntax_is_refused_with_the_fix() {
    let err = Pattern::parse("- /go").unwrap_err().to_string();
    assert!(err.contains("\"/go\""), "{err}");
    assert!(Pattern::parse("/Library/Pioneer/***").is_err());
    assert!(Pattern::parse("/").is_err());
}
