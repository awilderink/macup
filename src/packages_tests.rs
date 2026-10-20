use super::*;

fn found() -> Lookup {
    Lookup::Found {
        name: "x".into(),
        detail: "1.0".into(),
    }
}

fn unknown() -> Lookup {
    Lookup::Unknown("offline".into())
}

fn lookups(cask: Lookup, brew: Lookup, nix: Lookup) -> Vec<(Kind, Lookup)> {
    vec![(Kind::Cask, cask), (Kind::Brew, brew), (Kind::Nix, nix)]
}

// A name that is both a cask and a formula or nixpkgs package (docker, ollama,
// tailscale) is almost always meant as the cask on a Mac.
#[test]
fn a_cask_wins_over_a_formula_or_package_of_the_same_name() {
    assert_eq!(
        decide(&lookups(found(), found(), found())),
        Decision::Use(Kind::Cask)
    );
    assert_eq!(
        decide(&lookups(found(), Lookup::Missing, Lookup::Missing)),
        Decision::Use(Kind::Cask)
    );
}

// Formulae come from Homebrew when it has them, and from nixpkgs only
// otherwise.
#[test]
fn a_formula_wins_over_a_nixpkgs_package() {
    assert_eq!(
        decide(&lookups(Lookup::Missing, found(), found())),
        Decision::Use(Kind::Brew)
    );
    assert_eq!(
        decide(&lookups(unknown(), found(), Lookup::Missing)),
        Decision::Use(Kind::Brew)
    );
    assert_eq!(
        decide(&lookups(Lookup::Missing, Lookup::Missing, found())),
        Decision::Use(Kind::Nix)
    );
    assert_eq!(
        decide(&lookups(unknown(), unknown(), found())),
        Decision::Use(Kind::Nix)
    );
}

// Guessing while one source could not be checked and the others found nothing
// would quietly add the wrong kind of thing, so it must stop and ask for a
// flag instead.
#[test]
fn does_not_guess_when_a_lookup_failed_and_the_others_found_nothing() {
    let missing = || Lookup::Missing;
    assert_eq!(
        decide(&lookups(missing(), missing(), missing())),
        Decision::NotFound
    );
    assert_eq!(
        decide(&lookups(unknown(), missing(), missing())),
        Decision::CannotTell
    );
    assert_eq!(
        decide(&lookups(missing(), unknown(), missing())),
        Decision::CannotTell
    );
    assert_eq!(
        decide(&lookups(missing(), missing(), unknown())),
        Decision::CannotTell
    );
}
