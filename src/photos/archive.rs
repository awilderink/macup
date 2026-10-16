use super::library::{self, Asset};
use crate::volume::clonefile;
use anyhow::{Context, Result};
use chrono::{Local, TimeZone};
use std::collections::{HashMap, HashSet};
use std::fs::{File, FileTimes, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Debug, Default, PartialEq)]
pub struct Stats {
    pub added: usize,
    pub bytes: u64,
    pub total: usize,
}

pub fn manifest_path(archive: &Path) -> PathBuf {
    archive.join(".manifest")
}

pub fn load_manifest(archive: &Path) -> Result<HashMap<String, String>> {
    let mut map = HashMap::new();
    let Ok(f) = File::open(manifest_path(archive)) else {
        return Ok(map);
    };
    for line in BufReader::new(f).lines() {
        if let Some((src, dst)) = line?.split_once('\t') {
            map.insert(src.to_string(), dst.to_string());
        }
    }
    Ok(map)
}

pub fn archive_name(rel: &str, asset: Option<&Asset>) -> String {
    let file = rel.rsplit('/').next().unwrap_or(rel);
    let Some(asset) = asset else {
        return format!("unknown/{rel}");
    };
    let ext = Path::new(file)
        .extension()
        .map(|e| e.to_string_lossy().into_owned());
    let uuid = library::uuid_of(file).unwrap_or(file);
    let original = asset.original_name.as_deref().map(|n| n.replace('/', "_"));
    let primary = ext
        .as_deref()
        .is_some_and(|ext| file == format!("{uuid}.{ext}"));
    let name = match (&original, primary) {
        (Some(o), true) => o.clone(),
        (Some(o), false) => {
            let stem = Path::new(o)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| o.clone());
            match &ext {
                Some(ext) => format!("{stem}.{ext}"),
                None => stem,
            }
        }
        (None, _) => file.to_string(),
    };
    let folder = asset
        .created
        .and_then(|t| Local.timestamp_opt(t, 0).single())
        .map(|d| d.format("%Y/%Y-%m-%d").to_string())
        .unwrap_or_else(|| "undated".into());
    format!("{folder}/{name}")
}

fn with_suffix(name: &str, suffix: &str) -> String {
    let p = Path::new(name);
    match (p.file_stem(), p.extension()) {
        (Some(stem), Some(ext)) => {
            let parent = p
                .parent()
                .map(|d| d.display().to_string())
                .unwrap_or_default();
            let file = format!(
                "{} ({suffix}).{}",
                stem.to_string_lossy(),
                ext.to_string_lossy()
            );
            if parent.is_empty() {
                file
            } else {
                format!("{parent}/{file}")
            }
        }
        _ => format!("{name} ({suffix})"),
    }
}

fn same_size(a: &Path, b: &Path) -> bool {
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(x), Ok(y)) => x.len() == y.len(),
        _ => false,
    }
}

fn reusable(src: &Path, target: &Path, name: &str, claimed: &HashSet<String>) -> bool {
    !claimed.contains(name) && same_size(src, target)
}

pub fn update(
    archive: &Path,
    library: &Path,
    mut progress: impl FnMut(usize, usize),
) -> Result<Stats> {
    std::fs::create_dir_all(archive)?;
    let mut manifest = load_manifest(archive)?;
    let files = library::original_files(library)?;
    let pending: Vec<&String> = files
        .iter()
        .filter(|f| !manifest.contains_key(*f))
        .collect();
    if pending.is_empty() {
        return Ok(Stats {
            total: manifest.len(),
            ..Stats::default()
        });
    }
    let assets = library::assets(library)?;
    let mut claimed: HashSet<String> = manifest.values().cloned().collect();
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(manifest_path(archive))?;
    let mut stats = Stats::default();
    for (n, rel) in pending.iter().enumerate() {
        progress(n, pending.len());
        let src = library.join("originals").join(rel);
        let file = rel.rsplit('/').next().unwrap_or(rel);
        let asset = library::uuid_of(file).and_then(|u| assets.get(u));
        let mut name = archive_name(rel, asset);
        let mut target = archive.join(&name);
        if target.exists() && !reusable(&src, &target, &name, &claimed) {
            name = with_suffix(
                &name,
                library::uuid_of(file).map(|u| &u[..8]).unwrap_or(file),
            );
            target = archive.join(&name);
            if target.exists() && !reusable(&src, &target, &name, &claimed) {
                anyhow::bail!("cannot find a free name in the archive for {rel}");
            }
        }
        if !target.exists() {
            if let Some(dir) = target.parent() {
                std::fs::create_dir_all(dir)?;
            }
            clonefile(&src, &target)
                .context("the originals archive must be on the same drive as the library copy")?;
            if let Some(t) = asset.and_then(|a| a.created).filter(|t| *t > 0) {
                let when = SystemTime::UNIX_EPOCH + Duration::from_secs(t as u64);
                let _ = OpenOptions::new()
                    .write(true)
                    .open(&target)
                    .and_then(|f| f.set_times(FileTimes::new().set_modified(when)));
            }
            stats.added += 1;
            stats.bytes += std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
        }
        writeln!(log, "{rel}\t{name}")?;
        claimed.insert(name.clone());
        manifest.insert(rel.to_string(), name);
    }
    log.flush()?;
    stats.total = manifest.len();
    Ok(stats)
}

#[cfg(test)]
#[path = "archive_tests.rs"]
mod tests;
