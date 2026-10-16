use crate::env::Env;
use crate::errors::cancelled;
use crate::ui::{interactive, note, ok, select};
use crate::volume::Lock;
use anyhow::{Result, bail};

pub fn backed_up_macs(e: &Env) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(e.macs_dir()) else {
        return vec![];
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| !name.starts_with('.'))
        .collect();
    names.sort();
    names
}

pub fn has_legacy_layout(e: &Env) -> bool {
    e.volume.join("home").is_dir() || e.volume.join("photos").is_dir()
}

pub fn migrate_legacy(e: &Env) -> Result<()> {
    if !has_legacy_layout(e) {
        return Ok(());
    }
    let _lock = Lock::acquire(&e.lock_dir())?;
    let moves: Vec<_> = ["home", "photos"]
        .into_iter()
        .map(|name| (e.volume.join(name), e.machine_dir().join(name)))
        .filter(|(from, _)| from.is_dir())
        .collect();
    if let Some((from, to)) = moves.iter().find(|(_, to)| to.exists()) {
        bail!(
            "the drive has both the old layout ({}) and the new one ({}); move one of them aside by hand",
            from.display(),
            to.display()
        );
    }
    std::fs::create_dir_all(e.machine_dir())?;
    for (from, to) in &moves {
        std::fs::rename(from, to)?;
    }
    ok(format!(
        "moved the backup to {} so several Macs can share this drive (rename that folder if it came from another Mac)",
        e.machine_dir().display()
    ));
    Ok(())
}

pub fn source(e: &Env, from: Option<&str>) -> Result<Env> {
    let macs = backed_up_macs(e);
    if let Some(name) = from {
        if !macs.iter().any(|m| m == name) {
            bail!(
                "no backup from a Mac called {name:?} on this drive (found: {})",
                list(&macs)
            );
        }
        return Ok(e.for_machine(name));
    }
    if macs.contains(&e.machine) {
        return Ok(e.clone());
    }
    match macs.as_slice() {
        [] => bail!("no backups on {} yet", e.volume.display()),
        [only] => {
            note(format!(
                "this Mac ({}) has no backup of its own; using the one from {only}",
                e.machine
            ));
            Ok(e.for_machine(only))
        }
        _ if interactive() => {
            let options: Vec<(String, String)> =
                macs.iter().map(|m| (m.clone(), m.clone())).collect();
            let chosen = select("Restore from which Mac?", &options).map_err(|_| cancelled())?;
            Ok(e.for_machine(&chosen))
        }
        _ => bail!(
            "several Macs are backed up on this drive ({}); pick one with --from",
            list(&macs)
        ),
    }
}

fn list(macs: &[String]) -> String {
    if macs.is_empty() {
        "none".into()
    } else {
        macs.join(", ")
    }
}

#[cfg(test)]
#[path = "machines_tests.rs"]
mod tests;
