use crate::env::Env;
use crate::errors::is_cancelled;
use crate::photos;
use crate::restore::{self, Request};
use crate::ui::{confirm, human_bytes, interactive, note, ok, title, warn, with_spinner};
use crate::volume::{disk_usage, free_space};
use anyhow::{Result, bail};
use std::path::PathBuf;

pub fn run(e: &Env) -> Result<()> {
    if !interactive() {
        bail!(
            "macup setup asks what to bring back, so run it in a terminal; scripts can use `macup apply` and `macup restore --path …`"
        );
    }
    let src = crate::machines::open(e, None, true)?;
    let steps = if e.nix_enabled() { 3 } else { 2 };
    let mut done = 0;
    let mut heading = |label: &str| {
        done += 1;
        println!();
        println!("{}", title(format!("Step {done} of {steps} · {label}")));
    };
    if e.nix_enabled() {
        heading("Tools, apps and settings");
        crate::nix::install_and_apply(e, false, false, "macup setup")?;
    }
    heading("Files and folders");
    if src.backup_home().exists() {
        skippable(
            restore::run_from(&src, Request::pick(e.home.clone())),
            "`macup restore` brings files back later",
        )?;
    } else {
        note(format!("no files backed up from {}", src.machine));
    }
    heading("Photos");
    restore_photos(e, &src)?;
    println!();
    ok("This Mac is set up.");
    note("From now on, `macup backup` keeps the backup up to date.");
    Ok(())
}

fn skippable(result: Result<()>, later: &str) -> Result<()> {
    match result {
        Err(err) if is_cancelled(&err) => {
            note(format!("skipped; {later}"));
            Ok(())
        }
        other => other,
    }
}

fn restore_photos(e: &Env, src: &Env) -> Result<()> {
    let pictures = e.home.join("Pictures");
    let libs: Vec<PathBuf> = photos::libraries_in(&src.photos_dir())
        .into_iter()
        .filter(|l| l.file_name().is_some_and(|n| !pictures.join(n).exists()))
        .collect();
    if libs.is_empty() {
        note(
            "nothing to bring back: no Photos library in the backup, or it is already on this Mac",
        );
        return Ok(());
    }
    let names = libs
        .iter()
        .filter_map(|l| l.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(", ");
    let size: u64 = with_spinner("Measuring the Photos library…", || {
        Ok(libs.iter().filter_map(|l| disk_usage(l)).sum())
    })?;
    let free = free_space(&e.home).map(|(free, _)| free).unwrap_or(0);
    if size > free {
        warn(format!(
            "{names} needs {} but this Mac has only {} free",
            human_bytes(size),
            human_bytes(free)
        ));
        note("free up space, then run `macup restore --photos`");
        return Ok(());
    }
    if !confirm(
        &format!(
            "Put {names} back? It takes {} of the {} free on this Mac.",
            human_bytes(size),
            human_bytes(free)
        ),
        true,
    )? {
        note("skipped; `macup restore --photos` does it later");
        return Ok(());
    }
    photos::restore_photos(src, &pictures)
}
