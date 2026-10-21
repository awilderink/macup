use crate::env::Env;
use crate::volume::{check_mounted, unprepared_drives};
use crate::{apply, backup, config, errors, packages, restore, setup, status, ui};
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    UseDrive,
    Setup,
    Backup,
    Restore,
    CleanUp,
    Add,
    Apply,
    Update,
    Edit,
    Quit,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Drive {
    Ready,
    Unprepared(Vec<PathBuf>),
    Missing,
}

#[derive(Debug)]
pub struct Situation {
    pub drive: Drive,
    pub backup_on_drive: bool,
    pub last_backup: Option<Duration>,
    pub set_up: bool,
}

const DAY: Duration = Duration::from_secs(24 * 60 * 60);

pub fn recommend(s: &Situation) -> Option<(Action, String)> {
    match &s.drive {
        Drive::Missing => None,
        Drive::Unprepared(drives) => Some((
            Action::UseDrive,
            format!(
                "{} is connected but not set up for backups yet",
                drive_names(drives)
            ),
        )),
        Drive::Ready if s.backup_on_drive && !s.set_up => Some((
            Action::Setup,
            "this Mac looks freshly installed and the drive has a backup".into(),
        )),
        Drive::Ready if s.backup_on_drive && s.last_backup.is_none() => Some((
            Action::Restore,
            "bring back what you need before the first backup from this Mac".into(),
        )),
        Drive::Ready => match s.last_backup {
            None => Some((Action::Backup, "this Mac has never been backed up".into())),
            Some(age) if age >= DAY => {
                Some((Action::Backup, format!("the last backup was {}", ago(age))))
            }
            Some(_) => None,
        },
    }
}

fn ago(age: Duration) -> String {
    match age.as_secs() / DAY.as_secs() {
        1 => "yesterday".into(),
        days => format!("{days} days ago"),
    }
}

fn drive_names(drives: &[PathBuf]) -> String {
    drives
        .iter()
        .filter_map(|d| d.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" or ")
}

fn situation(e: &Env) -> Situation {
    let drive = if e.drive_config().is_file() && check_mounted(&e.volume).is_ok() {
        Drive::Ready
    } else {
        match unprepared_drives(Path::new("/Volumes")) {
            drives if drives.is_empty() => Drive::Missing,
            drives => Drive::Unprepared(drives),
        }
    };
    let backup_on_drive = drive == Drive::Ready
        && (e.backup_home().exists()
            || crate::machines::has_legacy_layout(e)
            || crate::machines::backed_up_macs(e).len() == 1);
    let last_backup = std::fs::metadata(e.last_backup_file())
        .and_then(|m| m.modified())
        .ok()
        .map(|t| SystemTime::now().duration_since(t).unwrap_or_default());
    let set_up = apply::installed(&e.config.install);
    Situation {
        drive,
        backup_on_drive,
        last_backup,
        set_up,
    }
}

fn options(s: &Situation) -> Vec<(String, Action)> {
    let mut all = Vec::new();
    if let Drive::Unprepared(drives) = &s.drive {
        all.push((
            format!("Use {} for backups", drive_names(drives)),
            Action::UseDrive,
        ));
    }
    if s.drive == Drive::Ready {
        all.extend([
            ("Back up this Mac".to_string(), Action::Backup),
            ("Restore files and folders".to_string(), Action::Restore),
            ("Set up this Mac from the backup".to_string(), Action::Setup),
        ]);
    }
    all.extend([
        ("Add a tool or app".to_string(), Action::Add),
        ("Install what macup.toml lists".to_string(), Action::Apply),
        ("Update all tools and apps".to_string(), Action::Update),
        ("Edit macup.toml".to_string(), Action::Edit),
    ]);
    if s.drive == Drive::Ready {
        all.push((
            "Clean up the backup (remove what you deleted on this Mac)".to_string(),
            Action::CleanUp,
        ));
    }
    all.push(("Quit".to_string(), Action::Quit));
    all
}

pub fn run(mut e: Env) -> Result<()> {
    if !ui::interactive() {
        use clap::CommandFactory;
        crate::Cli::command().print_help()?;
        return Ok(());
    }
    loop {
        println!();
        status::show(&e);
        println!();
        let s = situation(&e);
        let mut opts = options(&s);
        if s.drive == Drive::Missing {
            ui::warn("connect your backup drive to back up or restore");
        }
        if let Some((action, why)) = recommend(&s)
            && let Some(i) = opts.iter().position(|(_, a)| *a == action)
        {
            let picked = opts.remove(i);
            println!("{} {why}", ui::title("→"));
            opts.insert(0, picked);
        }
        let action = match ui::select("What do you want to do?", &opts) {
            Ok(Action::Quit) | Err(_) => return Ok(()),
            Ok(action) => action,
        };
        errors::report(match action {
            Action::UseDrive => use_drive(&s).and_then(|()| {
                e = Env::load()?;
                Ok(())
            }),
            Action::Setup => setup::run(&e),
            Action::Backup => backup::run(&e, &backup::Options::default()),
            Action::CleanUp => backup::run(
                &e,
                &backup::Options {
                    mirror: true,
                    ..backup::Options::default()
                },
            ),
            Action::Restore => restore::menu(&e),
            Action::Add => add(&e),
            Action::Apply => apply::apply(&e, false, false),
            Action::Update => apply::apply(&e, true, false),
            Action::Edit => apply::edit(&e),
            Action::Quit => Ok(()),
        });
    }
}

fn use_drive(s: &Situation) -> Result<()> {
    let Drive::Unprepared(drives) = &s.drive else {
        return Ok(());
    };
    let drive = match drives.as_slice() {
        [only] => only.clone(),
        _ => ui::select(
            "Which drive should hold the backups?",
            &drives
                .iter()
                .map(|d| (d.display().to_string(), d.clone()))
                .collect::<Vec<_>>(),
        )?,
    };
    ui::confirm_or_cancel(
        &format!(
            "Use {} for macup backups? This adds a {} file to it; nothing else changes.",
            drive.display(),
            config::FILE_NAME
        ),
        true,
    )?;
    config::init_drive(&drive)
}

fn add(e: &Env) -> Result<()> {
    let name = ui::input("Name of the tool or app (e.g. htop or firefox)")?;
    if name.trim().is_empty() {
        return Err(errors::cancelled());
    }
    packages::add(e, &[name.trim().to_string()], None)
}

#[cfg(test)]
#[path = "menu_tests.rs"]
mod tests;
