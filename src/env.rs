use crate::config::{self, Config};
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct Env {
    pub home: PathBuf,
    pub volume: PathBuf,
    pub machine: String,
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
            machine: env_string("MACUP_MACHINE", &this_mac_name()),
            volume,
        })
    }

    #[cfg(test)]
    pub fn new(home: PathBuf, volume: PathBuf) -> Env {
        Env {
            home,
            volume,
            machine: "test-mac".into(),
            config: Arc::default(),
        }
    }

    pub fn for_machine(&self, machine: &str) -> Env {
        Env {
            machine: machine.to_string(),
            ..self.clone()
        }
    }

    pub fn macs_dir(&self) -> PathBuf {
        self.volume.join("macs")
    }

    pub fn machine_dir(&self) -> PathBuf {
        self.macs_dir().join(&self.machine)
    }

    pub fn user(&self) -> String {
        self.home
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    pub fn backup_home(&self) -> PathBuf {
        self.machine_dir().join("home")
    }

    pub fn photos_dir(&self) -> PathBuf {
        self.machine_dir().join("photos")
    }

    pub fn archive_dir(&self) -> PathBuf {
        self.photos_dir().join("originals")
    }

    pub fn drive_nix_dir(&self) -> PathBuf {
        self.volume.join("nix-darwin")
    }

    pub fn nix_dir(&self) -> PathBuf {
        match self.config.nix.config.as_deref() {
            Some(dir) => match dir.strip_prefix("~/") {
                Some(rest) => self.home.join(rest),
                None => PathBuf::from(dir),
            },
            None => self.home.join(".config").join("nix-darwin"),
        }
    }

    pub fn nix_enabled(&self) -> bool {
        self.config.nix.enabled.unwrap_or_else(|| {
            self.has_nix_config()
                || has_flake(&self.drive_nix_dir())
                || crate::nix::find_nix().is_some()
        })
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

pub fn this_mac_name() -> String {
    let name = std::process::Command::new("scutil")
        .args(["--get", "LocalHostName"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    if name.is_empty() {
        "this-mac".into()
    } else {
        name
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
