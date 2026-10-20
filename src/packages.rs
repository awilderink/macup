use crate::config::{Config, InstallConfig};
use crate::env::{Env, has_flake};
use crate::nix::{drive_config, find_nix, nix_args, offer_apply, prepare};
use crate::search::{self, Lookup};
use crate::ui::{confirm, interactive, note, ok, warn, with_spinner};
use anyhow::{Context, Result, bail};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Brew,
    Cask,
    Nix,
}

const PREFERENCE: [Kind; 3] = [Kind::Cask, Kind::Brew, Kind::Nix];

impl Kind {
    fn key(self) -> &'static str {
        match self {
            Kind::Brew => "brew",
            Kind::Cask => "cask",
            Kind::Nix => "nix",
        }
    }

    fn from_key(key: &str) -> Kind {
        match key {
            "brew" => Kind::Brew,
            "cask" => Kind::Cask,
            _ => Kind::Nix,
        }
    }

    fn homebrew(self) -> Option<Homebrew> {
        match self {
            Kind::Brew => Some(Homebrew::Formula),
            Kind::Cask => Some(Homebrew::Cask),
            Kind::Nix => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Kind::Brew => "Homebrew formula",
            Kind::Cask => "Homebrew cask",
            Kind::Nix => "nixpkgs package",
        }
    }

    fn flag(self) -> &'static str {
        match self {
            Kind::Brew => "--brew",
            Kind::Cask => "--cask",
            Kind::Nix => "--nix",
        }
    }
}

struct ListsFile {
    path: PathBuf,
    src: String,
    upgraded: bool,
}

impl ListsFile {
    fn open(e: &Env) -> Result<ListsFile> {
        if let Some(dir) = e.own_flake() {
            bail!(
                "your tools and apps are in your own flake at {} ([nix] config in macup.toml); add or remove them there",
                dir.display()
            );
        }
        drive_config(e)?;
        let path = e.drive_config();
        let src = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        Ok(match crate::tools::upgrade(&src)? {
            Some(src) => ListsFile {
                path,
                src,
                upgraded: true,
            },
            None => ListsFile {
                path,
                src,
                upgraded: false,
            },
        })
    }

    fn install(&self) -> Result<InstallConfig> {
        Ok(Config::parse(&self.src)?.install)
    }

    fn edit(&mut self, kind: Kind, names: &[String], add: bool) -> Result<Vec<String>> {
        let (out, changed) = crate::tools::edit(&self.src, kind.key(), names, add)?;
        self.src = out;
        Ok(changed)
    }

    fn save(&self) -> Result<()> {
        Config::parse(&self.src)
            .with_context(|| format!("the edited {} would not be valid", self.path.display()))?;
        std::fs::write(&self.path, &self.src)
            .with_context(|| format!("writing {}", self.path.display()))?;
        if self.upgraded {
            note(format!(
                "moved your lists from [tools] to [install] in {}; apps is now called cask",
                self.path.display()
            ));
        }
        Ok(())
    }
}

pub fn upgrade_file(e: &Env) -> Result<()> {
    if e.own_flake().is_some() {
        return Ok(());
    }
    let file = ListsFile::open(e)?;
    if file.upgraded {
        file.save()?;
    }
    Ok(())
}

pub fn add(e: &Env, names: &[String], forced: Option<Kind>) -> Result<()> {
    let mut file = ListsFile::open(e)?;
    let listed = file.install()?;
    let _ = prepare(e);
    let mut picked: Vec<(Kind, String)> = Vec::new();
    for name in names {
        if let Some(installer) = listed.installer_of(name) {
            ok(format!(
                "{name} is already listed under install.{installer}"
            ));
            continue;
        }
        let (kind, name) = kind_of(e, name, forced)?;
        if let Some(installer) = listed.installer_of(&name) {
            ok(format!(
                "{name} is already listed under install.{installer}"
            ));
            continue;
        }
        if !picked.iter().any(|(_, n)| *n == name) {
            picked.push((kind, name));
        }
    }
    let mut added = Vec::new();
    for kind in PREFERENCE {
        let list: Vec<String> = picked
            .iter()
            .filter(|(k, _)| *k == kind)
            .map(|(_, name)| name.clone())
            .collect();
        if list.is_empty() {
            continue;
        }
        let changed = file.edit(kind, &list, true)?;
        if !changed.is_empty() {
            added.push(format!(
                "Added {} to install.{} in {}",
                changed.join(", "),
                kind.key(),
                file.path.display()
            ));
        }
    }
    if added.is_empty() {
        return Ok(());
    }
    file.save()?;
    added.iter().for_each(ok);
    offer_apply(e)
}

pub fn remove(e: &Env, names: &[String]) -> Result<()> {
    let mut file = ListsFile::open(e)?;
    let listed = file.install()?;
    let mut removed = Vec::new();
    let mut found: HashSet<String> = HashSet::new();
    let mut nix_changed = false;
    let mut uninstall: Vec<(String, Homebrew)> = Vec::new();
    for (key, list) in listed.lists() {
        let kind = Kind::from_key(key);
        let stored: Vec<String> = list
            .iter()
            .filter(|n| {
                names
                    .iter()
                    .any(|name| name.to_lowercase() == n.to_lowercase())
            })
            .cloned()
            .collect();
        let changed = file.edit(kind, &stored, false)?;
        if changed.is_empty() {
            continue;
        }
        removed.push(format!(
            "Removed {} from install.{} in {}",
            changed.join(", "),
            kind.key(),
            file.path.display()
        ));
        match kind.homebrew() {
            Some(brew) => uninstall.extend(changed.iter().map(|n| (n.clone(), brew))),
            None => nix_changed = true,
        }
        found.extend(changed.iter().map(|n| n.to_lowercase()));
    }
    if !removed.is_empty() {
        file.save()?;
    }
    removed.iter().for_each(ok);
    for name in names.iter().filter(|n| !found.contains(&n.to_lowercase())) {
        match installed_by_brew(name, &[Homebrew::Cask, Homebrew::Formula]) {
            Some(kind) => {
                note(format!(
                    "{name} is not in your lists, but Homebrew installed it"
                ));
                uninstall.push((name.clone(), kind));
            }
            None => warn(format!("{name} is not in your lists")),
        }
    }
    offer_uninstall(&uninstall)?;
    if nix_changed {
        return offer_apply(e);
    }
    Ok(())
}

pub fn check_edited(e: &Env, before: &InstallConfig, after: &InstallConfig) -> Result<()> {
    let new: Vec<(Kind, String)> = after
        .lists()
        .into_iter()
        .flat_map(|(key, list)| {
            let kept = before
                .lists()
                .into_iter()
                .find(|(k, _)| *k == key)
                .map(|(_, l)| l.clone())
                .unwrap_or_default();
            list.iter()
                .filter(move |name| !kept.contains(name))
                .map(move |name| (Kind::from_key(key), name.clone()))
        })
        .collect();
    if new.is_empty() {
        return Ok(());
    }
    let _ = prepare(e);
    let results: Vec<Lookup> = with_spinner("Checking the names you added…", || {
        Ok(std::thread::scope(|s| {
            let handles: Vec<_> = new
                .iter()
                .map(|(kind, name)| s.spawn(move || lookup(e, name, *kind)))
                .collect();
            handles
                .into_iter()
                .map(|h| {
                    h.join()
                        .unwrap_or_else(|_| Lookup::Unknown("the lookup crashed".into()))
                })
                .collect()
        }))
    })?;
    let mut missing = Vec::new();
    for ((kind, name), found) in new.iter().zip(results) {
        match found {
            Lookup::Missing => missing.push(format!("there is no {} called {name}", kind.label())),
            Lookup::Unknown(why) => warn(format!("could not check {name}: {why}")),
            Lookup::Found { name: current, .. } if current != *name => warn(format!(
                "Homebrew now calls {name} {current}; write {current} in install.{}",
                kind.key()
            )),
            Lookup::Found { .. } => {}
        }
    }
    if !missing.is_empty() {
        bail!("{}", missing.join("; "));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Homebrew {
    Cask,
    Formula,
}

impl Homebrew {
    fn flag(self) -> &'static str {
        match self {
            Homebrew::Cask => "--cask",
            Homebrew::Formula => "--formula",
        }
    }

    fn api(self) -> &'static str {
        match self {
            Homebrew::Cask => "cask",
            Homebrew::Formula => "formula",
        }
    }
}

fn installed_by_brew(name: &str, kinds: &[Homebrew]) -> Option<Homebrew> {
    if kinds.is_empty() {
        return None;
    }
    let brew = crate::rsync::which("brew")?;
    kinds.iter().copied().find(|kind| {
        Command::new(&brew)
            .args(["list", kind.flag(), name])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

fn offer_uninstall(items: &[(String, Homebrew)]) -> Result<()> {
    let Some(brew) = crate::rsync::which("brew") else {
        return Ok(());
    };
    for (name, kind) in items {
        if installed_by_brew(name, &[*kind]).is_none() {
            continue;
        }
        let flag = kind.flag();
        if !interactive() || !confirm(&format!("Also uninstall {name} from this Mac?"), true)? {
            note(format!(
                "{name} is still installed; `brew uninstall {flag} {name}` removes it"
            ));
            continue;
        }
        if !Command::new(&brew)
            .args(["uninstall", flag, name])
            .status()?
            .success()
        {
            bail!("Homebrew could not uninstall {name}");
        }
        ok(format!("Uninstalled {name}"));
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum Decision {
    Use(Kind),
    NotFound,
    CannotTell,
}

fn decide(lookups: &[(Kind, Lookup)]) -> Decision {
    if let Some((kind, _)) = lookups
        .iter()
        .find(|(_, lookup)| matches!(lookup, Lookup::Found { .. }))
    {
        return Decision::Use(*kind);
    }
    if lookups.iter().all(|(_, lookup)| *lookup == Lookup::Missing) {
        Decision::NotFound
    } else {
        Decision::CannotTell
    }
}

fn describe(asked: &str, kind: Kind, lookup: &Lookup) -> String {
    let Lookup::Found { name, detail } = lookup else {
        return asked.to_string();
    };
    if name != asked {
        note(format!(
            "Homebrew now calls {asked} {name}; adding it under that name"
        ));
    }
    match detail.is_empty() {
        true => note(format!("{name}: {}", kind.label())),
        false => note(format!("{name}: {} ({detail})", kind.label())),
    }
    name.clone()
}

fn lookup(e: &Env, name: &str, kind: Kind) -> Lookup {
    match kind {
        Kind::Brew => lookup_brew(Homebrew::Formula, name),
        Kind::Cask => lookup_brew(Homebrew::Cask, name),
        Kind::Nix => lookup_nix(e, name),
    }
}

fn kind_of(e: &Env, name: &str, forced: Option<Kind>) -> Result<(Kind, String)> {
    if let Some(kind) = forced {
        let found = with_spinner(&format!("Looking up {name}…"), || {
            Ok(lookup(e, name, kind))
        })?;
        return match &found {
            Lookup::Missing => bail!("{}", not_found(e, name, Some(kind))),
            Lookup::Unknown(why) => {
                warn(format!("could not check {name}: {why}"));
                Ok((kind, name.to_string()))
            }
            Lookup::Found { .. } => Ok((kind, describe(name, kind, &found))),
        };
    }
    let lookups: Vec<(Kind, Lookup)> = with_spinner(&format!("Looking up {name}…"), || {
        Ok(std::thread::scope(|s| {
            PREFERENCE
                .map(|kind| (kind, s.spawn(move || lookup(e, name, kind))))
                .into_iter()
                .map(|(kind, handle)| {
                    let found = handle.join().unwrap_or_else(|_| {
                        Lookup::Unknown(format!("the {} lookup crashed", kind.label()))
                    });
                    (kind, found)
                })
                .collect()
        }))
    })?;
    match decide(&lookups) {
        Decision::Use(kind) => {
            let mut preferred = true;
            let mut chosen = name.to_string();
            for (other, found) in &lookups {
                if *other == kind {
                    chosen = describe(name, kind, found);
                    preferred = false;
                    continue;
                }
                match found {
                    Lookup::Found { .. } => note(format!(
                        "there is also a {} called {name}; `macup add {} {name}` adds that instead",
                        other.label(),
                        other.flag()
                    )),
                    Lookup::Unknown(why) if preferred => note(format!(
                        "could not check for a {} called {name}: {why}",
                        other.label()
                    )),
                    _ => {}
                }
            }
            Ok((kind, chosen))
        }
        Decision::NotFound => bail!("{}", not_found(e, name, None)),
        Decision::CannotTell => {
            let why: Vec<String> = lookups
                .iter()
                .filter_map(|(kind, found)| match found {
                    Lookup::Unknown(why) => Some(format!("{}: {why}", kind.label())),
                    _ => None,
                })
                .collect();
            bail!(
                "could not tell which installer {name} needs ({}); say which with --cask, --brew or --nix",
                why.join("; ")
            )
        }
    }
}

fn nix_eval(e: &Env, nix: &Path, attr: &str) -> std::result::Result<String, String> {
    let mut args = vec!["eval".to_string(), "--raw".to_string()];
    if let Some(dir) = e
        .own_flake()
        .or(Some(e.flake_dir()))
        .filter(|d| has_flake(d))
    {
        args.extend(["--inputs-from".to_string(), dir.display().to_string()]);
    }
    args.push(attr.to_string());
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = Command::new(nix)
        .args(nix_args(&args))
        .output()
        .map_err(|err| err.to_string())?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).trim().to_string());
    }
    Err(String::from_utf8_lossy(&out.stderr)
        .lines()
        .filter(|l| !l.starts_with("fetching "))
        .collect::<Vec<_>>()
        .join("\n"))
}

fn lookup_nix(e: &Env, name: &str) -> Lookup {
    let Some(nix) = find_nix() else {
        return Lookup::Unknown("Nix is not installed yet".into());
    };
    match nix_eval(e, &nix, &format!("nixpkgs#{name}.name")) {
        Ok(found) => Lookup::Found {
            name: name.to_string(),
            detail: found,
        },
        Err(err) if err.contains("does not provide attribute") => Lookup::Missing,
        Err(err) => Lookup::Unknown(err),
    }
}

fn lookup_brew(kind: Homebrew, name: &str) -> Lookup {
    let Some(brew) = crate::rsync::which("brew") else {
        return search::brew_from_api(kind.api(), name);
    };
    let out = match Command::new(brew)
        .args(["info", kind.flag(), "--json=v2", name])
        .output()
    {
        Ok(out) => out,
        Err(err) => return Lookup::Unknown(err.to_string()),
    };
    if out.status.success() {
        return search::brew_found(name, &out.stdout);
    }
    let err = String::from_utf8_lossy(&out.stderr).to_lowercase();
    if err.contains("no cask with this name")
        || err.contains(&format!("no available {}", kind.api()))
    {
        Lookup::Missing
    } else {
        Lookup::Unknown(err.trim().to_string())
    }
}

fn not_found(e: &Env, name: &str, kind: Option<Kind>) -> String {
    let url = search::web_url(name);
    let wanted = |k: Kind| kind.is_none_or(|kind| kind == k);
    let (packages, formulae, casks) = with_spinner("Looking for similar names…", || {
        let packages = wanted(Kind::Nix)
            .then(|| {
                let release = find_nix()
                    .and_then(|nix| nix_eval(e, &nix, "nixpkgs#lib.trivial.release").ok());
                search::packages(release.as_deref(), name)
            })
            .flatten()
            .unwrap_or_default();
        let similar = |k: Kind, brew: Homebrew| {
            if wanted(k) {
                search::similar_brews(&e.home, brew.api(), name)
            } else {
                Vec::new()
            }
        };
        Ok((
            packages,
            similar(Kind::Brew, Homebrew::Formula),
            similar(Kind::Cask, Homebrew::Cask),
        ))
    })
    .unwrap_or_default();
    let mut msg = match kind {
        Some(kind) => format!("there is no {} called {name:?}.", kind.label()),
        None => format!("neither nixpkgs nor Homebrew has anything called {name:?}."),
    };
    if !packages.is_empty() {
        msg.push_str(&format!(
            "\nnixpkgs packages on search.nixos.org:\n{}",
            search::format(&packages)
        ));
    }
    if !formulae.is_empty() {
        msg.push_str(&format!(
            "\nHomebrew formulae:\n  {}",
            formulae.join("\n  ")
        ));
    }
    if !casks.is_empty() {
        msg.push_str(&format!("\nHomebrew casks:\n  {}", casks.join("\n  ")));
    }
    match kind {
        Some(Kind::Cask | Kind::Brew) if casks.is_empty() && formulae.is_empty() => {
            msg.push_str(&format!(" Try `brew search {name}`."))
        }
        Some(Kind::Cask | Kind::Brew) => {}
        _ => msg.push_str(&format!("\nMore at {url}")),
    }
    msg
}

#[cfg(test)]
#[path = "packages_tests.rs"]
mod tests;
