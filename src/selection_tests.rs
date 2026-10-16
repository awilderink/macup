use super::*;
use std::fs;

pub fn fixture_tree() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for f in [
        ".zshrc",
        ".ssh/id_ed25519",
        ".ssh/config",
        "Documents/a.txt",
        "Documents/b.txt",
        "Documents/sub/c.txt",
    ] {
        let p = root.path().join(f);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, f).unwrap();
    }
    root
}

#[test]
fn selecting_a_directory_covers_its_subtree() {
    let root = fixture_tree();
    let mut s = Selection::new(root.path());
    s.toggle("Documents").unwrap();
    assert_eq!(s.state("Documents/sub/c.txt"), CheckState::Checked);
    assert_eq!(s.sorted(), vec!["Documents"]);
}

#[test]
fn selecting_a_directory_drops_redundant_descendants() {
    let root = fixture_tree();
    let mut s = Selection::new(root.path());
    s.toggle("Documents/sub/c.txt").unwrap();
    s.toggle("Documents").unwrap();
    assert_eq!(s.sorted(), vec!["Documents"]);
}

// Unchecking one file inside a checked folder must keep its siblings (and the
// siblings of every folder on the way down) selected, so nothing else is lost.
#[test]
fn unchecking_inside_a_checked_directory_keeps_siblings() {
    let root = fixture_tree();
    let mut s = Selection::new(root.path());
    s.toggle("Documents").unwrap();
    s.toggle("Documents/sub/c.txt").unwrap();
    assert_eq!(s.sorted(), vec!["Documents/a.txt", "Documents/b.txt"]);
    assert_eq!(s.state("Documents"), CheckState::Partial);
    assert_eq!(s.state("Documents/sub"), CheckState::Unchecked);
}

// Selecting every child one by one should collapse to the parent so the rsync
// list stays minimal and new files in that folder are included too.
#[test]
fn selecting_all_children_collapses_to_parent() {
    let root = fixture_tree();
    let mut s = Selection::new(root.path());
    s.toggle(".ssh/id_ed25519").unwrap();
    s.toggle(".ssh/config").unwrap();
    assert_eq!(s.sorted(), vec![".ssh"]);
}

#[test]
fn top_level_never_collapses_into_root() {
    let root = fixture_tree();
    let mut s = Selection::new(root.path());
    for p in [".zshrc", ".ssh", "Documents"] {
        s.toggle(p).unwrap();
    }
    assert_eq!(s.sorted().len(), 3);
}

#[test]
fn toggle_twice_restores_empty_selection() {
    let root = fixture_tree();
    let mut s = Selection::new(root.path());
    s.toggle("Documents/a.txt").unwrap();
    s.toggle("Documents/a.txt").unwrap();
    assert!(s.sorted().is_empty());
}

#[test]
fn is_under_requires_a_path_boundary() {
    assert!(is_under("Library/Mail", "Library"));
    assert!(!is_under("LibraryX", "Library"));
    assert!(!is_under("Library", "Library"));
    assert!(is_under("a", ""));
}
