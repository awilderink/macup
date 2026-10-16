use super::*;
use crate::selection::tests::fixture_tree;

// Folders sort before files and dotfolders are shown, because a home backup's
// most valuable restore targets (.ssh, .claude) are hidden directories.
#[test]
fn lists_folders_first_including_dotfolders() {
    let root = fixture_tree();
    let p = Picker::new(root.path(), presets(&crate::config::Config::default())).unwrap();
    assert_eq!(p.row_names(), vec![".ssh", "Documents", ".zshrc"]);
}

#[test]
fn expand_select_and_confirm() {
    let root = fixture_tree();
    let mut p = Picker::new(root.path(), presets(&crate::config::Config::default())).unwrap();
    p.handle_key("down");
    p.handle_key("right");
    assert!(p.is_expanded("Documents"));
    p.handle_key("down");
    assert_eq!(p.cursor_rel(), "Documents/sub");
    p.handle_key(" ");
    assert_eq!(p.sel.sorted(), vec!["Documents/sub"]);
    assert_eq!(p.sel.state("Documents"), CheckState::Partial);
    p.handle_key("enter");
    assert!(p.confirmed && p.done);
}

#[test]
fn refuses_to_continue_with_nothing_selected() {
    let root = fixture_tree();
    let mut p = Picker::new(root.path(), presets(&crate::config::Config::default())).unwrap();
    p.handle_key("enter");
    assert!(!p.confirmed);
    assert!(p.status.contains("nothing selected"));
}

#[test]
fn preset_selects_only_paths_present_in_backup() {
    let root = fixture_tree();
    let mut p = Picker::new(root.path(), presets(&crate::config::Config::default())).unwrap();
    p.handle_key("p");
    p.handle_key("1");
    assert_eq!(p.sel.sorted(), vec![".zshrc"]);
}

#[test]
fn renders_without_panicking_on_a_small_terminal() {
    let root = fixture_tree();
    let mut p = Picker::new(root.path(), presets(&crate::config::Config::default())).unwrap();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(20, 5)).unwrap();
    terminal.draw(|f| p.render(f)).unwrap();
}
