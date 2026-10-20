use crate::config::{self, Config};
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Env {
    pub home: PathBuf,
    pub volume: PathBuf,
    pub machine: String,
    pub config: Config,
}

impl Env {
    pub fn load() -> Result<Env> {
        let volume = match env_path("MACUP_VOLUME") {
            Some(v) => v,
            None => detect_drive(Path::new("/Volumes"))?,
        };
        let home = env_path("MACUP_HOME")
            .or_else(|| env_path("HOME"))
            .unwrap_or_default();
        let machine = env_var("MACUP_MACHINE").unwrap_or_else(this_mac_name);
        let config = Config::load(&volume)?;
        Ok(Env::assemble(home, volume, machine, config))
    }

    fn assemble(home: PathBuf, volume: PathBuf, machine: String, config: Config) -> Env {
        Env {
            home,
            volume,
            machine,
            config,
        }
    }

    #[cfg(test)]
    pub fn new(home: PathBuf, volume: PathBuf) -> Env {
        Env::assemble(home, volume, "test-mac".into(), Config::default())
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

    pub fn backup_home(&self) -> PathBuf {
        self.machine_dir().join("home")
    }

    pub fn photos_dir(&self) -> PathBuf {
        self.machine_dir().join("photos")
    }

    pub fn archive_dir(&self) -> PathBuf {
        self.photos_dir().join("originals")
    }

    pub fn drive_config(&self) -> PathBuf {
        self.volume.join(config::FILE_NAME)
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.home.join("Library/Caches/macup")
    }

    pub fn rustup(&self) -> PathBuf {
        self.home.join(".cargo/bin/rustup")
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
}

pub fn this_mac_name() -> String {
    std::process::Command::new("scutil")
        .args(["--get", "LocalHostName"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "this-mac".into())
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

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

pub fn env_var(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

pub fn env_string(key: &str, fallback: &str) -> String {
    env_var(key).unwrap_or_else(|| fallback.to_string())
}
