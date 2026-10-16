use crate::config::{self, Config};
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct Env {
    pub home: PathBuf,
    pub volume: PathBuf,
    pub config: Arc<Config>,
}

impl Env {
    pub fn load() -> Result<Env> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let volume = match std::env::var_os("MACUP_VOLUME").filter(|v| !v.is_empty()) {
            Some(v) => PathBuf::from(v),
            None => detect_drive(Path::new("/Volumes"))?,
        };
        Ok(Env {
            home: env_or("MACUP_HOME", home),
            config: Arc::new(Config::load(&volume)?),
            volume,
        })
    }

    #[cfg(test)]
    pub fn new(home: PathBuf, volume: PathBuf) -> Env {
        Env {
            home,
            volume,
            config: Arc::default(),
        }
    }

    pub fn user(&self) -> String {
        self.home
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    pub fn backup_home(&self) -> PathBuf {
        self.volume.join("home")
    }

    pub fn photos_dir(&self) -> PathBuf {
        self.volume.join("photos")
    }

    pub fn archive_dir(&self) -> PathBuf {
        self.photos_dir().join("originals")
    }

    pub fn drive_nix_dir(&self) -> PathBuf {
        self.volume.join("nix-darwin")
    }

    pub fn nix_dir(&self) -> PathBuf {
        self.home.join(".config").join("nix-darwin")
    }

    pub fn log_dir(&self) -> PathBuf {
        self.home.join(".backup-logs")
    }

    pub fn lock_dir(&self) -> PathBuf {
        self.volume.join(".backup.lock")
    }

    pub fn last_backup_file(&self) -> PathBuf {
        self.log_dir().join("last-backup")
    }

    pub fn has_nix_config(&self) -> bool {
        has_flake(&self.nix_dir())
    }
}

pub fn detect_drive(volumes: &Path) -> Result<PathBuf> {
    let mut drives: Vec<PathBuf> = std::fs::read_dir(volumes)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.join(config::FILE_NAME).is_file())
                .collect()
        })
        .unwrap_or_default();
    drives.sort();
    match drives.len() {
        0 => Ok(volumes.join("Backup")),
        1 => Ok(drives.remove(0)),
        _ => bail!(
            "several backup drives are connected ({}); pick one with MACUP_VOLUME=/Volumes/<name>",
            drives
                .iter()
                .map(|d| d.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

pub fn env_or(key: &str, fallback: PathBuf) -> PathBuf {
    match std::env::var_os(key) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => fallback,
    }
}

pub fn env_string(key: &str, fallback: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

pub fn has_flake(dir: &Path) -> bool {
    dir.join("flake.nix").exists()
}
