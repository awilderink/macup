use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::path::Path;

pub const FILE_NAME: &str = "macup.toml";

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub backup: BackupConfig,
    pub restore: RestoreConfig,
    pub nix: NixConfig,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NixConfig {
    pub enabled: Option<bool>,
    pub config: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BackupConfig {
    pub rules: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RestoreConfig {
    pub presets: Vec<Preset>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preset {
    pub name: String,
    pub paths: Vec<String>,
}

impl Config {
    pub fn load(volume: &Path) -> Result<Config> {
        let path = volume.join(FILE_NAME);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Ok(Config::default());
        };
        let config: Config =
            toml::from_str(&text).with_context(|| format!("reading {}", path.display()))?;
        config
            .validate()
            .with_context(|| format!("in {}", path.display()))?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        for rule in &self.backup.rules {
            if !(rule.starts_with("+ ") || rule.starts_with("- ")) || rule.len() < 3 {
                bail!(
                    "backup rule {rule:?} must start with \"+ \" (include) or \"- \" (exclude), like the rsync filter rules in the README"
                );
            }
        }
        Ok(())
    }
}

pub const TEMPLATE: &str = "[backup]\nrules = []\n\n[restore]\npresets = []\n";

pub fn init_drive(drive: &Path) -> Result<()> {
    crate::volume::check_mounted(drive)?;
    let path = drive.join(FILE_NAME);
    if path.exists() {
        crate::ui::ok(format!("{} already exists", path.display()));
    } else {
        std::fs::write(&path, TEMPLATE)?;
        crate::ui::ok(format!(
            "{} is ready for backups; your own backup rules and restore presets go in {}",
            drive.display(),
            path.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
