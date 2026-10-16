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
    let folder_of = |a: &Asset| date_folder(a.created);
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
    format!("{}/{name}", folder_of(asset))
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

fn date_folder(created: Option<i64>) -> String {
    created
        .and_then(|t| Local.timestamp_opt(t, 0).single())
        .map(|d| d.format("%Y/%Y-%m-%d").to_string())
        .unwrap_or_else(|| "undated".into())
}

fn set_taken_time(path: &Path, created: Option<i64>) {
    if let Some(t) = created.filter(|t| *t > 0) {
        let when = SystemTime::UNIX_EPOCH + Duration::from_secs(t as u64);
        let _ = OpenOptions::new()
            .write(true)
            .open(path)
            .and_then(|f| f.set_times(FileTimes::new().set_modified(when)));
    }
}

pub struct Archive {
    dir: PathBuf,
    manifest: HashMap<String, String>,
    claimed: HashSet<String>,
    log: File,
}

impl Archive {
    pub fn open(dir: &Path) -> Result<Archive> {
        std::fs::create_dir_all(dir)?;
        let manifest = load_manifest(dir)?;
        let claimed = manifest.values().cloned().collect();
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(manifest_path(dir))?;
        Ok(Archive {
            dir: dir.to_path_buf(),
            manifest,
            claimed,
            log,
        })
    }

    pub fn contains(&self, key: &str) -> bool {
        self.manifest.contains_key(key)
    }

    pub fn len(&self) -> usize {
        self.manifest.len()
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn free_name(&self, name: String, key: &str, size: u64) -> Result<(String, bool)> {
        let file = key.rsplit('/').next().unwrap_or(key);
        let suffix = library::uuid_of(file).map(|u| &u[..8]).unwrap_or(file);
        for candidate in [name.clone(), with_suffix(&name, suffix)] {
            let target = self.dir.join(&candidate);
            match std::fs::metadata(&target) {
                Err(_) => return Ok((candidate, false)),
                Ok(meta) if !self.claimed.contains(&candidate) && meta.len() == size => {
                    return Ok((candidate, true));
                }
                Ok(_) => continue,
            }
        }
        anyhow::bail!("cannot find a free name in the archive for {key}")
    }

    fn record(&mut self, key: &str, name: String) -> Result<()> {
        writeln!(self.log, "{key}\t{name}")?;
        self.claimed.insert(name.clone());
        self.manifest.insert(key.to_string(), name);
        Ok(())
    }

    pub fn add_clone(
        &mut self,
        key: &str,
        src: &Path,
        asset: Option<&Asset>,
    ) -> Result<Option<u64>> {
        let size = std::fs::metadata(src)?.len();
        let (name, existing) = self.free_name(archive_name(key, asset), key, size)?;
        let target = self.dir.join(&name);
        if !existing {
            std::fs::create_dir_all(target.parent().expect("archive files live in a folder"))?;
            clonefile(src, &target)
                .context("the originals archive must be on the same drive as the library copy")?;
            set_taken_time(&target, asset.and_then(|a| a.created));
        }
        self.record(key, name)?;
        Ok((!existing).then_some(size))
    }

    pub fn add_downloaded(
        &mut self,
        key: &str,
        file: &Path,
        original_name: &str,
        created: Option<i64>,
    ) -> Result<u64> {
        let size = std::fs::metadata(file)?.len();
        let name = format!(
            "{}/{}",
            date_folder(created),
            original_name.replace('/', "_")
        );
        let (name, existing) = self.free_name(name, key, size)?;
        let target = self.dir.join(&name);
        if existing {
            std::fs::remove_file(file)?;
        } else {
            std::fs::create_dir_all(target.parent().expect("archive files live in a folder"))?;
            std::fs::rename(file, &target)?;
            set_taken_time(&target, created);
        }
        self.record(key, name)?;
        Ok(size)
    }

    pub fn flush(&mut self) -> Result<()> {
        Ok(self.log.flush()?)
    }
}

pub fn update(
    archive: &Path,
    library: &Path,
    mut progress: impl FnMut(usize, usize),
) -> Result<Stats> {
    let mut a = Archive::open(archive)?;
    let files = library::original_files(library)?;
    let pending: Vec<&String> = files.iter().filter(|f| !a.contains(f)).collect();
    let mut stats = Stats::default();
    if !pending.is_empty() {
        let assets = library::assets(library)?;
        for (n, rel) in pending.iter().enumerate() {
            progress(n, pending.len());
            let file = rel.rsplit('/').next().unwrap_or(rel);
            let asset = library::uuid_of(file).and_then(|u| assets.get(u));
            if let Some(bytes) = a.add_clone(rel, &library.join("originals").join(rel), asset)? {
                stats.added += 1;
                stats.bytes += bytes;
            }
        }
        a.flush()?;
    }
    stats.total = a.len();
    Ok(stats)
}

#[cfg(test)]
#[path = "archive_tests.rs"]
mod tests;
