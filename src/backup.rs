use crate::access::full_disk_access_guard;
use crate::copy::{Job, Spec, log_path, plan};
use crate::env::Env;
use crate::guard::{Check, Guard, precomputed, resolve};
use crate::photos::{self, PhotoLibrary};
use crate::rules::Filter;
use crate::ui::{human_bytes, note, plural, report_result, run_job, step, warn, with_spinner};
use crate::volume::{Lock, free_space, nearly_full};
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

pub fn backup_spec(e: &Env, o: &Options) -> Result<Spec> {
    Ok(Spec {
        src: e.home.clone(),
        dst: e.backup_home(),
        filter: Filter::backup(&e.config, &o.excludes)?,
        mirror: o.mirror,
        ..Spec::default()
    })
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

pub fn mirror_guard(e: &Env, o: &Options) -> Result<Option<Guard>> {
    let spec = backup_spec(e, o)?;
    let plan = with_spinner("Checking what mirror mode would delete…", || {
        Ok(plan(&spec))
    })?;
    let count = plan.deletions().count();
    let samples: Vec<&str> = plan.deletions().take(8).collect();
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
    crate::machines::prepare(e, !o.dry_run)?;
    if o.no_photos && o.photos_only {
        bail!("--no-photos and --photos-only cannot be combined");
    }
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
            checks.push(Box::new(|| mirror_guard(e, o)));
        }
    }
    resolve(checks, o.yes || o.dry_run)?;
    let _lock = Lock::acquire(&e.lock_dir())?;
    if !o.photos_only {
        backup_home(e, o)?;
    }
    for lib in &libs {
        if photos_open {
            if lib.dst.exists() {
                photos::archive_library(e, &lib.dst)?;
            }
        } else {
            photos::backup_library(e, o.dry_run, lib)?;
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

fn backup_home(e: &Env, o: &Options) -> Result<()> {
    std::fs::create_dir_all(e.backup_home())?;
    let spec = backup_spec(e, o)?;
    if o.dry_run {
        return preview(e, &spec);
    }
    let mut title = format!(
        "Backing up {} → {}",
        e.home.display(),
        e.backup_home().display()
    );
    if o.mirror {
        title.push_str(" (mirror)");
    }
    let res = run_job(
        &title,
        &Job {
            spec,
            log_path: log_path(e, "backup"),
            strict: false,
        },
    )?;
    report_result("Backup", &res)?;
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
    Ok(())
}

fn preview(e: &Env, spec: &Spec) -> Result<()> {
    let plan = with_spinner("Comparing this Mac with the backup…", || Ok(plan(spec)))?;
    let log = log_path(e, "backup-preview");
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&log, plan.describe().join("\n") + "\n")?;
    step("Backup preview (dry run)");
    let files = plan.files().count();
    note(format!(
        "{} to copy ({})",
        plural(files, "file", "files"),
        human_bytes(plan.bytes())
    ));
    let deleted = plan.deletions().count();
    if deleted > 0 {
        note(format!(
            "{} to delete from the backup",
            plural(deleted, "file or folder", "files and folders")
        ));
    }
    for problem in plan.problems.iter().take(5) {
        warn(problem);
    }
    note(format!("the full list is in {}", log.display()));
    Ok(())
}

pub fn last_backup(e: &Env) -> String {
    if let Ok(s) = std::fs::read_to_string(e.last_backup_file()) {
        return s.trim().to_string();
    }
    match std::fs::metadata(e.backup_home()).and_then(|m| m.modified()) {
        Ok(t) => format!(
            "none from this Mac yet (the drive's copy last changed {})",
            chrono::DateTime::<chrono::Local>::from(t).format("%Y-%m-%d %H:%M")
        ),
        Err(_) => "none yet".into(),
    }
}

#[cfg(test)]
#[path = "backup_tests.rs"]
pub mod tests;
