use super::*;

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn add(src: &str, key: &str, list: &[&str]) -> (String, Vec<String>) {
    edit(src, key, &names(list), true).unwrap()
}

fn remove(src: &str, key: &str, list: &[&str]) -> (String, Vec<String>) {
    edit(src, key, &names(list), false).unwrap()
}

const CONFIG: &str = "# my drive\n[backup]\nrules = [\"- /go\"] # skip Go\n\n[install]\nbrew = [\n  \"bat\",\n  # search\n  \"ripgrep\",\n]\ncask = [\"ghostty\"]\n";

// Each item goes on its own line in a multi-line list, in alphabetical order,
// so hand-written lists keep the shape their author chose.
#[test]
fn adds_in_order_keeping_a_multi_line_list_multi_line() {
    let (out, changed) = add(CONFIG, "brew", &["fd", "zoxide", "aria2"]);
    assert_eq!(changed, names(&["fd", "zoxide", "aria2"]));
    assert!(
        out.contains("brew = [\n  \"aria2\",\n  \"bat\",\n  \"fd\",\n  # search\n  \"ripgrep\",\n  \"zoxide\",\n]"),
        "{out}"
    );
}

#[test]
fn adds_to_a_one_line_list_on_the_same_line() {
    let (out, _) = add(CONFIG, "cask", &["brave-browser", "raycast"]);
    assert!(
        out.contains("cask = [\"brave-browser\", \"ghostty\", \"raycast\"]"),
        "{out}"
    );
}

// Comments and everything outside the list must survive an edit; macup.toml
// also holds the backup rules.
#[test]
fn keeps_the_rest_of_the_file() {
    let (out, _) = add(CONFIG, "brew", &["fd"]);
    let (back, _) = remove(&out, "brew", &["fd"]);
    assert_eq!(back, CONFIG);
}

#[test]
fn creates_the_section_and_list_when_missing() {
    let (out, changed) = add("[backup]\nrules = []\n", "brew", &["bat"]);
    assert_eq!(changed, names(&["bat"]));
    let config = crate::config::Config::parse(&out).unwrap();
    assert_eq!(config.install.brew, names(&["bat"]));
    let (out, _) = add(&out, "cask", &["ghostty"]);
    let config = crate::config::Config::parse(&out).unwrap();
    assert_eq!(config.install.cask, names(&["ghostty"]));
}

#[test]
fn removing_the_last_item_leaves_an_empty_list() {
    let (out, changed) = remove(CONFIG, "cask", &["ghostty"]);
    assert_eq!(changed, names(&["ghostty"]));
    let config = crate::config::Config::parse(&out).unwrap();
    assert!(config.install.cask.is_empty());
}

#[test]
fn reports_only_what_changed() {
    let (out, changed) = add(CONFIG, "brew", &["bat", "fd"]);
    assert_eq!(changed, names(&["fd"]));
    let (_, changed) = remove(&out, "brew", &["nope", "fd"]);
    assert_eq!(changed, names(&["fd"]));
}

#[test]
fn a_list_that_is_not_a_list_is_an_error() {
    assert!(edit("[install]\nbrew = \"bat\"\n", "brew", &names(&["fd"]), true).is_err());
    assert!(edit("install = 1\n", "brew", &names(&["fd"]), true).is_err());
}

const OLD: &str = "[backup]\nrules = []\n\n# what this Mac needs\n[tools]\nnix = [\"curl\", \"rustup\"]\n# apps from Homebrew\napps = [\n  \"ghostty\",\n]\n\n[nix]\nenabled = true\n";

// A drive written by an older macup must keep working, and its comments must
// survive the move to the current layout.
#[test]
fn upgrades_an_old_layout_in_place() {
    let out = upgrade(OLD).unwrap().unwrap();
    assert!(out.contains("# what this Mac needs\n[install]\n"), "{out}");
    assert!(
        out.contains("# apps from Homebrew\ncask = [\n  \"ghostty\",\n]"),
        "{out}"
    );
    for gone in ["[tools]", "apps =", "nix", "curl"] {
        assert!(!out.contains(gone), "{gone} left in {out}");
    }
    let config = crate::config::Config::parse(&out).unwrap();
    assert_eq!(config.install.cask, names(&["ghostty"]));
    assert_eq!(config.install.brew, names(&["rust"]));
    assert_eq!(upgrade(&out).unwrap(), None);
}

#[test]
fn an_old_layout_still_loads() {
    let config = crate::config::Config::parse(OLD).unwrap();
    assert_eq!(config.install.brew, names(&["rust"]));
    assert_eq!(config.install.cask, names(&["ghostty"]));
}

// Rust used to be its own setting, installed with rustup; it now comes from
// Homebrew like everything else, so the setting becomes the rust formula.
#[test]
fn the_rust_setting_becomes_the_rust_formula() {
    let out = upgrade("[install]\nbrew = [\"bat\", \"ripgrep\"]\nrust = true\n")
        .unwrap()
        .unwrap();
    let config = crate::config::Config::parse(&out).unwrap();
    assert_eq!(config.install.brew, names(&["bat", "ripgrep", "rust"]));
    assert!(!out.contains("rust = "), "{out}");
    let out = upgrade("[install]\nbrew = [\"Rust\"]\nrust = true\n")
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::config::Config::parse(&out).unwrap().install.brew,
        names(&["Rust"])
    );
    let out = upgrade("[install]\nrust = false\n").unwrap().unwrap();
    assert!(
        crate::config::Config::parse(&out)
            .unwrap()
            .install
            .is_empty()
    );
}

// A drive whose backup rules were written for rsync gets them rewritten as
// skip and keep, one per line, with the rest of the file untouched.
#[test]
fn upgrades_rsync_rules_to_skip_and_keep() {
    let old = "# my drive\n[backup]\nrules = [\n  \"- /go\",\n  \"+ /Library/Pioneer/\",\n  \"+ /Library/Pioneer/***\",\n  \"- *.iso\",\n]\n\n[install]\nbrew = [\"bat\"]\n";
    let err = upgrade(old).unwrap_err().to_string();
    assert!(err.contains("*.iso"), "{err}");
    let old = old.replace("  \"- *.iso\",\n", "");
    let out = upgrade(&old).unwrap().unwrap();
    assert!(out.starts_with("# my drive\n[backup]\n"), "{out}");
    assert!(out.contains("skip = [\n  \"/go\",\n]"), "{out}");
    assert!(
        out.contains("keep = [\n  \"/Library/Pioneer\",\n]"),
        "{out}"
    );
    assert!(!out.contains("rules"), "{out}");
    let config = crate::config::Config::parse(&out).unwrap();
    assert_eq!(config.backup.keep, names(&["/Library/Pioneer"]));
    assert_eq!(config.install.brew, names(&["bat"]));
}
