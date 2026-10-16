use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Env {
    pub home: PathBuf,
    pub volume: PathBuf,
}

impl Env {
    pub fn load() -> Env {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        Env {
            home: env_or("MACUP_HOME", home),
            volume: env_or("MACUP_VOLUME", PathBuf::from("/Volumes/Backup")),
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
