use super::*;

const HOME_NIX: &str = "{ lib, pkgs, ... }:\n\n{\n  home.packages = with pkgs; [\n    bat\n    curl\n    zoxide\n  ];\n\n  programs.home-manager.enable = true;\n}\n";

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn add_package_keeps_list_sorted_and_indented() {
    let (out, changed) = edit_packages(HOME_NIX, &names(&["go", "bat"]), true).unwrap();
    assert_eq!(changed, vec!["go"]);
    assert!(
        out.contains("    bat\n    curl\n    go\n    zoxide\n  ];"),
        "{out}"
    );
    assert!(out.contains("programs.home-manager.enable = true;"));
}

#[test]
fn remove_package() {
    let (out, changed) = edit_packages(HOME_NIX, &names(&["curl", "nope"]), false).unwrap();
    assert_eq!(changed, vec!["curl"]);
    assert!(!out.contains("curl"));
}

#[test]
fn edit_casks_rewrites_the_single_line_list() {
    let (out, _) = edit_casks(
        r#"casks = [ "brave-browser" "ghostty" ];"#,
        &names(&["firefox"]),
        true,
    )
    .unwrap();
    assert_eq!(out, r#"casks = [ "brave-browser" "firefox" "ghostty" ];"#);
}

// The real config must stay parseable by this editor; otherwise `macup add`
// would fail on the user's actual config even though the unit cases pass.
#[test]
fn real_config_is_editable() {
    let dir = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join(".config/nix-darwin");
    for (file, edit) in [
        ("home.nix", edit_packages as Edit),
        ("flake.nix", edit_casks as Edit),
    ] {
        let Ok(src) = std::fs::read_to_string(dir.join(file)) else {
            return;
        };
        let (_, changed) = edit(&src, &names(&["macup-test-package"]), true).unwrap();
        assert_eq!(changed.len(), 1, "{file}");
    }
}
