pub mod archive;
pub mod library;
pub mod photokit;

use crate::env::Env;
use crate::rsync::{Job, dir_arg, dry_run, find_rsync, log_path, rsync_args};
use crate::ui::{
    human_bytes, note, ok, plural, report_result, run_job, step, warn, warning, with_spinner,
};
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

pub fn icloud_only(e: &Env, lib: &PhotoLibrary) -> Result<Vec<(String, String)>> {
    let manifest = archive::load_manifest(&e.archive_dir())?;
    let originals = lib.src.join("originals");
    Ok(library::primary_files(&lib.src)?
        .into_iter()
        .filter(|(_, rel)| !manifest.contains_key(rel) && !originals.join(rel).exists())
        .collect())
}

fn download_key(rel: &str, uuid: &str, kind: photokit::Kind, name: &str) -> String {
    let dir = rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let ext = Path::new(name)
        .extension()
        .map(|x| x.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match kind {
        photokit::Kind::Primary => rel.to_string(),
        photokit::Kind::PairedVideo => format!("{dir}/{uuid}_3.{ext}"),
        photokit::Kind::Alternate => format!("{dir}/{uuid}_alternate.{ext}"),
        photokit::Kind::Audio => format!("{dir}/{uuid}_audio.{ext}"),
    }
}

pub fn fetch_icloud(e: &Env, lib: &PhotoLibrary, fetch: bool, dry_run: bool) -> Result<()> {
    let missing = icloud_only(e, lib)?;
    if missing.is_empty() {
        return Ok(());
    }
    warn(format!(
        "{} only in iCloud: neither this Mac nor the originals archive has them",
        plural(missing.len(), "original is", "originals are")
    ));
    if dry_run {
        return Ok(());
    }
    if !fetch
        && !(crate::ui::interactive()
            && crate::ui::confirm(
                "Download them from iCloud into the originals archive now?",
                true,
            )?)
    {
        note("download them later with `macup backup --photos-only --fetch-icloud`");
        return Ok(());
    }
    if photokit::access() == photokit::Access::NotAsked {
        note("macOS will ask once to give your terminal access to Photos");
        photokit::request_access()?;
    }
    if photokit::access() != photokit::Access::Granted {
        bail!(
            "no access to Photos; allow it in System Settings → Privacy & Security → Photos, then try again"
        );
    }
    let assets = library::assets(&lib.src)?;
    let mut archive = archive::Archive::open(&e.archive_dir())?;
    let incoming = archive.dir().to_path_buf();
    let (mut done, mut failed, mut bytes) = (0, 0, 0);
    for (n, (uuid, rel)) in missing.iter().enumerate() {
        step(format!("[{}/{}] downloading {}", n + 1, missing.len(), rel));
        let created = assets.get(uuid).and_then(|a| a.created);
        match photokit::download_originals(uuid, &incoming) {
            Ok(files) => {
                for f in files {
                    let key = download_key(rel, uuid, f.kind, &f.original_name);
                    bytes += archive.add_downloaded(&key, &f.path, &f.original_name, created)?;
                }
                done += 1;
            }
            Err(err) => {
                failed += 1;
                warn(format!("{rel}: {err:#}"));
            }
        }
    }
    archive.flush()?;
    ok(format!(
        "downloaded {} into the originals archive ({})",
        plural(done, "original", "originals"),
        human_bytes(bytes)
    ));
    if failed > 0 {
        bail!(
            "{} could not be downloaded; run the backup again to retry",
            plural(failed, "original", "originals")
        );
    }
    Ok(())
}

pub fn report(e: &Env, test_icloud: bool) -> Result<()> {
    let libs = photo_libraries(e);
    if libs.is_empty() {
        bail!("no Photos library in {}", e.home.join("Pictures").display());
    }
    let manifest = archive::load_manifest(&e.archive_dir())?;
    for lib in &libs {
        let primaries = library::primary_files(&lib.src)?;
        let originals = lib.src.join("originals");
        let local = primaries
            .iter()
            .filter(|(_, rel)| originals.join(rel).exists())
            .count();
        let archived = primaries
            .iter()
            .filter(|(_, rel)| manifest.contains_key(rel))
            .count();
        let only_icloud = icloud_only(e, lib)?.len();
        step(format!("{} · {} items", lib.name, primaries.len()));
        note(format!("{local} originals on this Mac"));
        note(format!(
            "{archived} in the originals archive ({} files in total, including Live Photo videos)",
            manifest.len()
        ));
        if only_icloud > 0 {
            warn(format!(
                "{only_icloud} only in iCloud — `macup backup --photos-only --fetch-icloud` downloads them"
            ));
        } else {
            ok("every original is on this Mac or in the archive");
        }
    }
    let access = photokit::access();
    note(format!(
        "Photos access for iCloud downloads: {}",
        match access {
            photokit::Access::Granted => "granted",
            photokit::Access::NotAsked => "not asked yet",
            photokit::Access::Denied => "denied (System Settings → Privacy & Security → Photos)",
        }
    ));
    if test_icloud {
        test_download(e, &libs[0], &manifest)?;
    }
    Ok(())
}

fn test_download(
    e: &Env,
    lib: &PhotoLibrary,
    manifest: &std::collections::HashMap<String, String>,
) -> Result<()> {
    let primaries = library::primary_files(&lib.src)?;
    let Some((uuid, rel, archived)) = primaries.iter().find_map(|(uuid, rel)| {
        manifest
            .get(rel)
            .map(|a| (uuid, rel, e.archive_dir().join(a)))
    }) else {
        bail!("nothing archived yet to compare against; run a backup first");
    };
    if photokit::access() == photokit::Access::NotAsked {
        note("macOS will ask once to give your terminal access to Photos");
        photokit::request_access()?;
    }
    if photokit::access() != photokit::Access::Granted {
        bail!("no access to Photos; allow it in System Settings → Privacy & Security → Photos");
    }
    let dir = std::env::temp_dir().join(format!("macup-photokit-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let result = with_spinner(
        &format!("Asking Photos for the original of {rel}…"),
        || photokit::download_originals(uuid, &dir),
    );
    let outcome = result.and_then(|files| {
        let primary = files
            .iter()
            .find(|f| f.kind == photokit::Kind::Primary)
            .ok_or_else(|| anyhow::anyhow!("Photos returned no primary original"))?;
        if std::fs::read(&primary.path)? != std::fs::read(&archived)? {
            bail!(
                "Photos returned {} but it differs from the archived copy {}",
                primary.original_name,
                archived.display()
            );
        }
        ok(format!(
            "PhotoKit works: {} came back byte-identical to {}",
            primary.original_name,
            archived.display()
        ));
        Ok(())
    });
    let _ = std::fs::remove_dir_all(&dir);
    outcome
}

pub fn last_photo_backup(e: &Env) -> String {
    let libs = photo_libraries(e);
    if libs.is_empty() {
        return "no library on this Mac".into();
    }
    let named = libs.len() > 1;
    let mut parts: Vec<String> = libs
        .iter()
        .map(|l| {
            let label = if named {
                format!("{}: ", l.name.trim_end_matches(".photoslibrary"))
            } else {
                String::new()
            };
            match std::fs::metadata(library::db_path(&l.dst)).and_then(|m| m.modified()) {
                Ok(t) => format!(
                    "{label}backed up {}",
                    chrono::DateTime::<chrono::Local>::from(t).format("%Y-%m-%d %H:%M")
                ),
                Err(_) => format!("{label}not backed up yet"),
            }
        })
        .collect();
    if let Ok(manifest) = archive::load_manifest(&e.archive_dir())
        && !manifest.is_empty()
    {
        parts.push(format!("{} originals kept on the drive", manifest.len()));
    }
    let only_icloud: usize = libs
        .iter()
        .filter_map(|l| icloud_only(e, l).ok())
        .map(|m| m.len())
        .sum();
    if only_icloud > 0 {
        parts.push(warning(format!("{only_icloud} only in iCloud")).to_string());
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
    let bin = find_rsync()?;
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
