use crate::apply::{drive_config, find_brew, offer_apply};
use crate::config::{Config, InstallConfig, Kind};
use crate::env::Env;
use crate::search::{self, Lookup};
use crate::ui::{confirm, interactive, note, ok, warn, with_spinner};
use anyhow::{Context, Result, bail};
use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;

const PREFERENCE: [Kind; 2] = [Kind::Cask, Kind::Brew];

struct ListsFile {
    path: PathBuf,
    src: String,
    upgraded: bool,
}

impl ListsFile {
    fn open(e: &Env) -> Result<ListsFile> {
        drive_config(e)?;
        let path = e.drive_config();
        let src = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let (src, upgraded) = match crate::tools::upgrade(&src)? {
            Some(upgraded) => (upgraded, true),
            None => (src, false),
        };
        Ok(ListsFile {
            path,
            src,
            upgraded,
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
                "updated {} to the current layout: the lists are under [install], apps is now cask, and the nix list is gone (Rust is now the rust formula under brew; git and curl come with macOS)",
                self.path.display()
            ));
        }
        Ok(())
    }
}

pub fn upgrade_file(e: &Env) -> Result<()> {
    let file = ListsFile::open(e)?;
    if file.upgraded {
        file.save()?;
    }
    Ok(())
}

pub fn add(e: &Env, names: &[String], forced: Option<Kind>) -> Result<()> {
    let mut file = ListsFile::open(e)?;
    let listed = file.install()?;
    let mut picked: Vec<(Kind, String)> = Vec::new();
    for name in names {
        if let Some(installer) = listed.installer_of(name) {
            ok(format!(
                "{name} is already listed under install.{}",
                installer.key()
            ));
            continue;
        }
        let (kind, name) = kind_of(e, name, forced)?;
        if let Some(installer) = listed.installer_of(&name) {
            ok(format!(
                "{name} is already listed under install.{}",
                installer.key()
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
    let mut uninstall: Vec<(String, Kind)> = Vec::new();
    for (kind, list) in listed.lists() {
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
        uninstall.extend(changed.iter().map(|n| (n.clone(), kind)));
        found.extend(changed.iter().map(|n| n.to_lowercase()));
    }
    if !removed.is_empty() {
        file.save()?;
    }
    removed.iter().for_each(ok);
    for name in names.iter().filter(|n| !found.contains(&n.to_lowercase())) {
        match installed_by_brew(name, &PREFERENCE) {
            Some(kind) => {
                note(format!(
                    "{name} is not in your lists, but Homebrew installed it"
                ));
                uninstall.push((name.clone(), kind));
            }
            None => warn(format!("{name} is not in your lists")),
        }
    }
    offer_uninstall(&uninstall)
}

pub fn check_edited(before: &InstallConfig, after: &InstallConfig) -> Result<()> {
    let new: Vec<(Kind, String)> = after
        .lists()
        .into_iter()
        .flat_map(|(kind, list)| {
            let kept = before
                .lists()
                .into_iter()
                .find(|(k, _)| *k == kind)
                .map(|(_, l)| l.clone())
                .unwrap_or_default();
            list.iter()
                .filter(move |name| !kept.contains(name))
                .map(move |name| (kind, name.clone()))
        })
        .collect();
    if new.is_empty() {
        return Ok(());
    }
    let results = with_spinner("Checking the names you added…", || Ok(lookup_all(&new)))?;
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

fn installed_by_brew(name: &str, kinds: &[Kind]) -> Option<Kind> {
    let brew = find_brew()?;
    kinds.iter().copied().find(|kind| {
        Command::new(&brew)
            .args(["list", kind.brew_flag(), name])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

fn offer_uninstall(items: &[(String, Kind)]) -> Result<()> {
    let Some(brew) = find_brew() else {
        return Ok(());
    };
    for (name, kind) in items {
        if installed_by_brew(name, &[*kind]).is_none() {
            continue;
        }
        let flag = kind.brew_flag();
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

fn lookup_all<S: AsRef<str> + Sync>(wanted: &[(Kind, S)]) -> Vec<Lookup> {
    std::thread::scope(|s| {
        let handles: Vec<_> = wanted
            .iter()
            .map(|(kind, name)| s.spawn(move || lookup_brew(*kind, name.as_ref())))
            .collect();
        handles
            .into_iter()
            .zip(wanted)
            .map(|(h, (kind, _))| {
                h.join().unwrap_or_else(|_| {
                    Lookup::Unknown(format!("the {} lookup crashed", kind.label()))
                })
            })
            .collect()
    })
}

fn kind_of(e: &Env, name: &str, forced: Option<Kind>) -> Result<(Kind, String)> {
    if let Some(kind) = forced {
        let found = with_spinner(&format!("Looking up {name}…"), || {
            Ok(lookup_brew(kind, name))
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
    let wanted = PREFERENCE.map(|kind| (kind, name));
    let lookups: Vec<(Kind, Lookup)> = with_spinner(&format!("Looking up {name}…"), || {
        Ok(PREFERENCE.into_iter().zip(lookup_all(&wanted)).collect())
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
                "could not tell whether {name} is a cask or a formula ({}); say which with --cask or --brew",
                why.join("; ")
            )
        }
    }
}

fn lookup_brew(kind: Kind, name: &str) -> Lookup {
    let Some(brew) = find_brew() else {
        return search::brew_from_api(kind.api(), name);
    };
    let out = match Command::new(brew)
        .args(["info", kind.brew_flag(), "--json=v2", name])
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
    let wanted = |k: Kind| kind.is_none_or(|kind| kind == k);
    let similar = |k: Kind| {
        if wanted(k) {
            search::similar_brews(&e.home, k.api(), name)
        } else {
            Vec::new()
        }
    };
    let (formulae, casks) = (similar(Kind::Brew), similar(Kind::Cask));
    let mut msg = match kind {
        Some(kind) => format!("there is no {} called {name:?}.", kind.label()),
        None => format!("Homebrew has no formula or cask called {name:?}."),
    };
    if !formulae.is_empty() {
        msg.push_str(&format!(
            "\nHomebrew formulae:\n  {}",
            formulae.join("\n  ")
        ));
    }
    if !casks.is_empty() {
        msg.push_str(&format!("\nHomebrew casks:\n  {}", casks.join("\n  ")));
    }
    if formulae.is_empty() && casks.is_empty() {
        msg.push_str(&format!(" Try `brew search {name}`."));
    }
    msg
}

#[cfg(test)]
#[path = "packages_tests.rs"]
mod tests;
