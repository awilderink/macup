use super::*;

fn hits(body: &str) -> Vec<Hit> {
    serde_json::from_str::<Response>(body).unwrap().into_hits()
}

// A real response from search.nixos.org, so a change in its shape shows up
// here rather than as a silently missing suggestion list.
#[test]
fn reads_a_real_search_response() {
    let found = hits(include_str!("../tests/fixtures/search-ripgrep.json"));
    assert_eq!(found.len(), 4);
    assert_eq!(found[0].name, "ripgrep");
    assert!(!found[0].version.is_empty());
    assert!(!found[0].description.is_empty());
}

// The backend searches several indexes for one channel at once, so the same
// package comes back more than once.
#[test]
fn lists_each_package_once_and_tolerates_missing_fields() {
    let body = r#"{"_shards":{"total":4},"hits":{"hits":[
        {"_source":{"package_attr_name":"ripgrep","package_pversion":"15.2.0","package_description":"Fast grep"}},
        {"_source":{"package_attr_name":"ripgrep","package_pversion":"15.2.0","package_description":"Fast grep"}},
        {"_source":{"package_attr_name":"rgx","package_description":null}}
    ]}}"#;
    let found = hits(body);
    assert_eq!(
        found.iter().map(|h| h.name.as_str()).collect::<Vec<_>>(),
        ["ripgrep", "rgx"]
    );
    assert_eq!(found[1].version, "");
}

#[test]
fn formats_aligned_lines_and_shortens_long_descriptions() {
    let out = format(&[
        Hit {
            name: "ripgrep".into(),
            version: "15.2.0".into(),
            description: "Fast grep\nsecond line".into(),
        },
        Hit {
            name: "rgx".into(),
            version: "0.1".into(),
            description: "x".repeat(200),
        },
    ]);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "  ripgrep  15.2.0  Fast grep");
    assert!(lines[1].starts_with("  rgx      0.1     xxx"), "{out}");
    assert!(
        lines[1].ends_with('…') && lines[1].chars().count() == 101,
        "{out}"
    );
}

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
