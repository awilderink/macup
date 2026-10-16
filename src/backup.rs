use crate::access::full_disk_access_guard;
use crate::env::Env;
use crate::guard::{Check, Guard, precomputed, resolve};
use crate::photos::{self, PhotoLibrary};
use crate::rsync::{Job, dir_arg, dry_run, find_rsync, log_path, rsync_args};
use crate::rules::write_rules_file;
use crate::ui::{human_bytes, plural, report_result, run_job, select, warn, with_spinner};
use crate::volume::{Lock, check_mounted, free_space, nearly_full};
use anyhow::{Result, bail};

#[derive(Debug, Default, Clone)]
pub struct Options {
    pub dry_run: bool,
    pub mirror: bool,
    pub yes: bool,
    pub no_photos: bool,
    pub photos_only: bool,
    pub fetch_icloud: bool,
    pub excludes: Vec<String>,
}

pub fn menu(e: &Env) -> Result<()> {
    let mirror = select(
        "How should the backup treat files you deleted on this Mac?",
        &[
            ("Keep them in the backup (safe, recommended)".into(), false),
            ("Delete them from the backup too (mirror)".into(), true),
        ],
    )?;
    let dry_run = select(
        "Preview first?",
        &[
            ("Back up for real".into(), false),
            ("Preview only (dry run)".into(), true),
        ],
    )?;
    run(
        e,
        &Options {
            dry_run,
            mirror,
            ..Options::default()
        },
    )
}

pub fn backup_args(e: &Env, o: &Options, rules: &str) -> Vec<String> {
    let mut args = rsync_args(Vec::<String>::new());
    if o.mirror {
        args.push("--delete".into());
    }
    args.extend(o.excludes.iter().map(|x| format!("--exclude={x}")));
    args.push(format!("--filter=merge {rules}"));
    args.push(dir_arg(&e.home));
    args.push(dir_arg(&e.backup_home()));
    args
}

pub fn has_backed_up_before(e: &Env) -> bool {
    e.last_backup_file().exists()
}

pub fn first_backup_guard(e: &Env) -> Option<Guard> {
    if has_backed_up_before(e) {
        return None;
    }
    let has_data = std::fs::read_dir(e.backup_home())
        .map(|mut d| d.next().is_some())
        .unwrap_or(false);
    has_data.then(|| {
        Guard::new(
            "This Mac has never been backed up with macup, but the drive already holds a backup. Backing up now replaces files in it with this Mac's versions; on a freshly installed Mac, default settings and empty app data would overwrite your old ones. Restore what you need first (macup restore).",
            "Back up anyway? Only if you have restored everything you want to keep.",
        )
    })
}

pub fn mirror_guard(e: &Env, o: &Options, bin: &str, rules: &str) -> Result<Option<Guard>> {
    let mut count = 0;
    let mut samples = Vec::new();
    with_spinner("Checking what mirror mode would delete…", || {
        let mut args = backup_args(e, o, rules);
        args.extend(["--existing".to_string(), "--ignore-existing".to_string()]);
        dry_run(bin, &args, |it| {
            if it.deletion() {
                count += 1;
                if samples.len() < 8 {
                    samples.push(it.name);
                }
            }
        })
    })?;
    if count == 0 {
        return Ok(None);
    }
    Ok(Some(Guard::new(
        format!(
            "Mirror mode would delete {} from {}, for example:\n    {}",
            plural(count, "file or folder", "files and folders"),
            e.backup_home().display(),
            samples.join("\n    ")
        ),
        format!(
            "Delete {} from the backup? They cannot be restored afterwards.",
            plural(count, "item", "items")
        ),
    )))
}

pub fn run(e: &Env, o: &Options) -> Result<()> {
    check_mounted(&e.volume)?;
    crate::machines::migrate_legacy(e)?;
    if o.no_photos && o.photos_only {
        bail!("--no-photos and --photos-only cannot be combined");
    }
    let bin = find_rsync(e)?;
    let rules = write_rules_file(&e.config)?;
    let rules_arg = rules.arg();
    let libs: Vec<PhotoLibrary> = if o.no_photos {
        vec![]
    } else {
        photos::photo_libraries(e)
    };
    let photos_open = !libs.is_empty() && !o.dry_run && photos::photos_app_running();
    if photos_open {
        warn(
            "Photos is open, so the Photos library copy is skipped this time (the originals archive is still updated); quit Photos and run `macup backup --photos-only` for it",
        );
    }
    let mut checks: Vec<Check> = vec![precomputed(full_disk_access_guard(e, "backup"))];
    if !o.photos_only {
        checks.push(precomputed(first_backup_guard(e)));
        if o.mirror {
            checks.push(Box::new(|| mirror_guard(e, o, &bin, &rules_arg)));
        }
    }
    resolve(checks, o.yes || o.dry_run)?;
    let _lock = Lock::acquire(&e.lock_dir())?;
    if !o.photos_only {
        backup_home(e, o, &bin, &rules_arg)?;
    }
    for lib in &libs {
        if photos_open {
            if lib.dst.exists() {
                photos::archive_library(e, &lib.dst)?;
            }
        } else {
            photos::backup_library(e, o.dry_run, &bin, lib)?;
        }
        photos::fetch_icloud(e, lib, o.fetch_icloud, o.dry_run)?;
    }
    if let Ok((free, total)) = free_space(&e.volume)
        && nearly_full(free, total)
    {
        warn(format!(
            "backup drive is {}% full ({} free)",
            100 - free * 100 / total,
            human_bytes(free)
        ));
    }
    Ok(())
}

fn backup_home(e: &Env, o: &Options, bin: &str, rules: &str) -> Result<()> {
    std::fs::create_dir_all(e.backup_home())?;
    let mut title = format!(
        "Backing up {} → {}",
        e.home.display(),
        e.backup_home().display()
    );
    let mut args = backup_args(e, o, rules);
    if o.dry_run {
        title = "Previewing backup (dry run)".into();
        args.push("--dry-run".into());
    } else if o.mirror {
        title.push_str(" (mirror)");
    }
    let res = run_job(
        &title,
        &Job {
            bin: bin.to_string(),
            args,
            log_path: log_path(e, "backup"),
            strict: false,
        },
    )?;
    report_result("Backup", &res)?;
    if o.dry_run {
        return Ok(());
    }
    let stamp = format!(
        "{} · {}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M"),
        res.summary()
    );
    if let Err(err) = std::fs::create_dir_all(e.log_dir())
        .and_then(|_| std::fs::write(e.last_backup_file(), stamp))
    {
        warn(format!("could not record the backup time: {err}"));
    }
    if let Err(err) = crate::nix::sync_config_to_drive(e, bin) {
        warn(format!(
            "could not copy the Nix config to the drive: {err:#}"
        ));
    }
    Ok(())
}

pub fn last_backup(e: &Env) -> String {
    if let Ok(s) = std::fs::read_to_string(e.last_backup_file()) {
        return s.trim().to_string();
    }
    match std::fs::metadata(e.backup_home()).and_then(|m| m.modified()) {
        Ok(t) => format!(
            "never from this Mac (backup folder last changed {})",
            chrono::DateTime::<chrono::Local>::from(t).format("%Y-%m-%d %H:%M")
        ),
        Err(_) => "none".into(),
    }
}

#[cfg(test)]
#[path = "backup_tests.rs"]
pub mod tests;
