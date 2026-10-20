use super::*;

#[test]
fn reads_the_version_from_brew_and_from_the_api() {
    let brew =
        br#"{"formulae":[],"casks":[{"token":"rekordbox","version":"7.2.19,20260914095016"}]}"#;
    assert_eq!(brew_version(brew).as_deref(), Some("7.2.19"));
    let api = br#"{"token":"firefox","version":"144.0"}"#;
    assert_eq!(brew_version(api).as_deref(), Some("144.0"));
    let brew = br#"{"formulae":[{"name":"bun","versions":{"stable":"1.4.3"}}],"casks":[]}"#;
    assert_eq!(brew_version(brew).as_deref(), Some("1.4.3"));
    let api = br#"{"name":"bun","versions":{"stable":"1.4.3","head":null}}"#;
    assert_eq!(brew_version(api).as_deref(), Some("1.4.3"));
}

// `macup add` records the name Homebrew uses now, so a renamed cask such as
// docker (now docker-desktop) is listed under the name that keeps working.
#[test]
fn reads_the_current_name_from_brew() {
    let brew = br#"{"formulae":[],"casks":[{"token":"docker-desktop","old_tokens":["docker"],"version":"4.50.0,1"}]}"#;
    assert_eq!(
        brew_found("docker", brew),
        Lookup::Found {
            name: "docker-desktop".into(),
            detail: "4.50.0".into()
        }
    );
    let formula = br#"{"formulae":[{"name":"ripgrep","versions":{"stable":"15.2.0"}}],"casks":[]}"#;
    assert!(matches!(brew_found("rg", formula), Lookup::Found { name, .. } if name == "ripgrep"));
    assert!(matches!(brew_found("bat", b"not json"), Lookup::Found { name, .. } if name == "bat"));
}

#[test]
fn suggests_close_and_containing_names() {
    let names = [
        "firefox",
        "firefox@beta",
        "firefly",
        "brave-browser",
        "rekordbox",
    ];
    assert_eq!(similar("firefx", names.into_iter()), ["firefox", "firefly"]);
    assert_eq!(
        similar("firefox", names.into_iter()),
        ["firefly", "firefox@beta"]
    );
    assert!(similar("zzqqxx", names.into_iter()).is_empty());
}
