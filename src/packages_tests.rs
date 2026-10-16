use super::*;

const HOME_NIX: &str = "{ lib, pkgs, ... }:\n\n{\n  home.packages = with pkgs; [\n    bat\n    curl\n    zoxide\n  ];\n\n  programs.home-manager.enable = true;\n}\n";

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn edit(src: &str, spec: &ListSpec, list: &[&str], add: bool) -> (String, Vec<String>) {
    edit_list(src, spec, &names(list), add)
        .unwrap()
        .expect("list found")
}

#[test]
fn adds_in_alphabetical_place_with_the_existing_indentation() {
    let (out, changed) = edit(HOME_NIX, &PACKAGES, &["go", "bat", "aria2"], true);
    assert_eq!(changed, vec!["go", "aria2"]);
    assert!(
        out.contains("    aria2\n    bat\n    curl\n    go\n    zoxide\n  ];"),
        "{out}"
    );
    assert!(out.contains("programs.home-manager.enable = true;"));
}

#[test]
fn removes_only_the_named_package() {
    let (out, changed) = edit(HOME_NIX, &PACKAGES, &["curl", "nope"], false);
    assert_eq!(changed, vec!["curl"]);
    assert!(out.contains("    bat\n    zoxide\n  ];"), "{out}");
}

// The old editor rewrote the whole list, dropping anything it did not
// understand; editing in place must keep comments and odd formatting intact.
#[test]
fn keeps_comments_and_surrounding_code() {
    let src = "{ pkgs, ... }: {\n  home.packages = with pkgs; [\n    bat # cat with wings\n    # editors\n    neovim\n  ] ++ lib.optional true jq;\n}\n";
    let (out, _) = edit(src, &PACKAGES, &["htop"], true);
    assert!(
        out.contains("bat # cat with wings\n    htop\n    # editors\n    neovim"),
        "{out}"
    );
    assert!(out.contains("] ++ lib.optional true jq;"));
}

#[test]
fn handles_nested_attribute_sets_and_pkgs_prefixes() {
    let src = "{ pkgs, ... }: {\n  home = {\n    stateVersion = \"26.05\";\n    packages = [ pkgs.bat pkgs.jq ];\n  };\n}\n";
    let (out, _) = edit(src, &PACKAGES, &["fd"], true);
    assert!(
        out.contains("packages = [ pkgs.bat pkgs.fd pkgs.jq ];"),
        "{out}"
    );
}

#[test]
fn edits_homebrew_casks_inside_a_module() {
    let src = "{\n  outputs = { ... }: {\n    modules = [ ({ config, ... }: {\n      homebrew = {\n        enable = true;\n        casks = [ \"brave-browser\" \"ghostty\" ];\n      };\n    }) ];\n  };\n}\n";
    let (out, _) = edit(src, &CASKS, &["firefox"], true);
    assert!(
        out.contains("casks = [ \"brave-browser\" \"firefox\" \"ghostty\" ];"),
        "{out}"
    );
    let (out, _) = edit(&out, &CASKS, &["brave-browser"], false);
    assert!(
        out.contains("casks = [ \"firefox\" \"ghostty\" ];"),
        "{out}"
    );
}

#[test]
fn fills_an_empty_list() {
    let (out, _) = edit(
        "{ pkgs, ... }: { home.packages = with pkgs; [ ]; }",
        &PACKAGES,
        &["jq"],
        true,
    );
    assert!(out.contains("[ jq ]"), "{out}");
}

#[test]
fn a_file_without_the_list_is_not_a_match() {
    assert!(
        edit_list("{ x = 1; }", &PACKAGES, &names(&["jq"]), true)
            .unwrap()
            .is_none()
    );
}

// The real config must stay editable; otherwise `macup add` would fail on the
// user's actual files even though the cases above pass.
#[test]
fn real_config_is_editable() {
    let dir = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join(".config/nix-darwin");
    for (file, spec) in [("home.nix", &PACKAGES), ("flake.nix", &CASKS)] {
        let Ok(src) = std::fs::read_to_string(dir.join(file)) else {
            return;
        };
        let (out, changed) = edit(&src, spec, &["macup-test-package"], true);
        assert_eq!(changed.len(), 1, "{file}");
        let (back, _) = edit(&out, spec, &["macup-test-package"], false);
        assert_eq!(back, src, "{file}: add then remove did not round-trip");
    }
}

// A file that does not parse must say so; otherwise the user gets a
// misleading "could not find the list" message.
#[test]
fn a_file_that_does_not_parse_is_an_error() {
    let err = edit_list(
        "{ home.packages = with pkgs; [ bat ",
        &PACKAGES,
        &names(&["jq"]),
        true,
    )
    .unwrap_err();
    assert!(err.to_string().contains("cannot parse"), "{err}");
}
