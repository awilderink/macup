pub mod archive;
pub mod library;

use crate::env::Env;
use crate::rsync::{Job, dir_arg, dry_run, find_rsync, log_path, rsync_args};
use crate::ui::{human_bytes, note, ok, plural, report_result, run_job, step, warn, with_spinner};
use crate::volume::{clonefile, rename_swap};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

pub struct PhotoLibrary {
    pub name: String,
    pub src: PathBuf,
    pub dst: PathBuf,
}

impl PhotoLibrary {
    pub fn staging(&self) -> PathBuf {
        self.dst.with_file_name(format!(".{}.staging", self.name))
    }
}

pub fn libraries_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut libs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "photoslibrary") && p.is_dir())
        .collect();
    libs.sort();
    libs
}

pub fn photo_libraries(e: &Env) -> Vec<PhotoLibrary> {
    libraries_in(&e.home.join("Pictures"))
        .into_iter()
        .map(|src| {
            let name = src
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            PhotoLibrary {
                dst: e.photos_dir().join(&name),
                name,
                src,
            }
        })
        .collect()
}

pub fn photos_app_running() -> bool {
    std::process::Command::new("pgrep")
        .args(["-x", "Photos"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn photo_args(src: &Path, dst: &Path) -> Vec<String> {
    rsync_args([
        "--delete".to_string(),
        "--filter=- /database/Photos.sqlite*".to_string(),
        "--filter=H /resources/cpl/".to_string(),
        dir_arg(src),
        dir_arg(dst),
    ])
}

pub fn archive_library(e: &Env, library: &Path) -> Result<()> {
    let stats = with_spinner("Adding originals to the archive…", || {
        archive::update(&e.archive_dir(), library, |_, _| {})
    })
    .context("updating the originals archive")?;
    if stats.added > 0 {
        ok(format!(
            "originals archive: {} added ({}), {} in total",
            plural(stats.added, "file", "files"),
            human_bytes(stats.bytes),
            stats.total
        ));
    }
    Ok(())
}

pub fn backup_library(e: &Env, dry_run_only: bool, bin: &str, lib: &PhotoLibrary) -> Result<()> {
    let src_items =
        library::item_count(&lib.src).with_context(|| format!("cannot read {}", lib.name))?;
    if dry_run_only {
        return preview(bin, lib, src_items);
    }
    let replacing = lib.dst.exists();
    if replacing {
        archive_library(e, &lib.dst)?;
    }
    let staging = lib.staging();
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    std::fs::create_dir_all(e.photos_dir())?;
    if replacing {
        clonefile(&lib.dst, &staging).context("could not stage the Photos backup")?;
    }
    if let Err(err) = stage(e, bin, lib, &staging, src_items) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(err);
    }
    if replacing {
        if let Err(err) = rename_swap(&staging, &lib.dst) {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(err.context("could not swap in the new Photos backup"));
        }
        std::fs::remove_dir_all(&staging)?;
    } else {
        std::fs::rename(&staging, &lib.dst)?;
    }
    archive_library(e, &lib.dst)
}

fn stage(e: &Env, bin: &str, lib: &PhotoLibrary, staging: &Path, src_items: u64) -> Result<()> {
    let db = library::db_path(staging);
    std::fs::create_dir_all(db.parent().expect("database has a parent"))?;
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", db.display()));
    }
    with_spinner(
        "Taking a consistent snapshot of the Photos database…",
        || library::snapshot(&lib.src, &db),
    )?;
    let job = Job {
        bin: bin.to_string(),
        args: photo_args(&lib.src, staging),
        log_path: log_path(e, "photos"),
        strict: true,
    };
    let res = run_job(
        &format!("Backing up {} → {}", lib.name, lib.dst.display()),
        &job,
    )?;
    report_result("Photos copy", &res)?;
    verify(staging, src_items)?;
    ok(format!(
        "Photos library {} backed up and verified ({src_items} items) in {}s",
        lib.name,
        res.duration.as_secs()
    ));
    Ok(())
}

pub fn verify(library_dir: &Path, want: u64) -> Result<()> {
    let check = library::quick_check(library_dir)?;
    if check != "ok" {
        bail!(
            "the copied Photos database failed its integrity check: {}",
            crate::ui::truncate(&check, 200)
        );
    }
    let got = library::item_count(library_dir)?;
    if got != want {
        bail!(
            "the copied Photos database has {got} items, expected {want} (was Photos still syncing? try again)"
        );
    }
    Ok(())
}

fn preview(bin: &str, lib: &PhotoLibrary, src_items: u64) -> Result<()> {
    let (mut files, mut deleted, mut bytes) = (0, 0, 0);
    dry_run(bin, &photo_args(&lib.src, &lib.dst), |it| {
        if it.deletion() {
            deleted += 1;
        } else if it.file() {
            files += 1;
            bytes += it.size;
        }
    })?;
    step(format!("Photos library {} ({src_items} items)", lib.name));
    note(format!(
        "{} to copy ({}), {} to remove from the library copy (the originals archive keeps them), plus the database",
        plural(files, "file", "files"),
        human_bytes(bytes),
        plural(deleted, "file or folder", "files and folders"),
    ));
    Ok(())
}

pub fn last_photo_backup(e: &Env) -> String {
    let libs = photo_libraries(e);
    if libs.is_empty() {
        return "no library on this Mac".into();
    }
    let mut parts: Vec<String> = libs
        .iter()
        .map(
            |l| match std::fs::metadata(library::db_path(&l.dst)).and_then(|m| m.modified()) {
                Ok(t) => format!(
                    "{}: {}",
                    l.name,
                    chrono::DateTime::<chrono::Local>::from(t).format("%Y-%m-%d %H:%M")
                ),
                Err(_) => format!("{}: never", l.name),
            },
        )
        .collect();
    if let Ok(manifest) = archive::load_manifest(&e.archive_dir())
        && !manifest.is_empty()
    {
        parts.push(format!("archive: {} originals", manifest.len()));
    }
    parts.join(" · ")
}

pub fn restore_photos(e: &Env, target: &Path) -> Result<()> {
    let libs = libraries_in(&e.photos_dir());
    if libs.is_empty() {
        bail!(
            "no Photos library backed up in {}",
            e.photos_dir().display()
        );
    }
    let bin = find_rsync(e)?;
    for src in libs {
        let name = src
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let dst = target.join(&name);
        if dst.exists() {
            warn(format!(
                "{} already exists; move it aside first if you want the backup copy instead",
                dst.display()
            ));
            continue;
        }
        std::fs::create_dir_all(target)?;
        let job = Job {
            bin: bin.clone(),
            args: rsync_args([dir_arg(&src), dir_arg(&dst)]),
            log_path: log_path(e, "restore"),
            strict: false,
        };
        let res = run_job(&format!("Restoring {name} → {}", dst.display()), &job)?;
        report_result("Photos restore", &res)?;
        note(
            "open Photos while holding Option to pick this library, then make it the System Photo Library in Photos → Settings",
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests.rs"]
pub mod tests;
