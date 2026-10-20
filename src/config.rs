use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::path::Path;

pub const FILE_NAME: &str = "macup.toml";

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub backup: BackupConfig,
    pub restore: RestoreConfig,
    pub install: InstallConfig,
    pub nix: NixConfig,
    #[serde(skip)]
    pub old_tools_section: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InstallConfig {
    pub brew: Vec<String>,
    pub cask: Vec<String>,
    pub nix: Vec<String>,
}

pub const INSTALLERS: [&str; 3] = ["brew", "cask", "nix"];

impl InstallConfig {
    pub fn lists(&self) -> [(&'static str, &Vec<String>); 3] {
        [
            ("brew", &self.brew),
            ("cask", &self.cask),
            ("nix", &self.nix),
        ]
    }

    pub fn is_empty(&self) -> bool {
        self.lists().iter().all(|(_, list)| list.is_empty())
    }

    pub fn installer_of(&self, name: &str) -> Option<&'static str> {
        let name = name.to_lowercase();
        self.lists()
            .into_iter()
            .find(|(_, list)| list.iter().any(|n| n.to_lowercase() == name))
            .map(|(installer, _)| installer)
    }

    fn validate(&self) -> Result<()> {
        let mut seen: Vec<(String, &str)> = Vec::new();
        for (installer, list) in self.lists() {
            for name in list {
                if name.trim().is_empty() {
                    bail!("install.{installer} has an empty name");
                }
                if installer != "nix" && name.contains('/') {
                    bail!("{name}: Homebrew taps other than Homebrew's own aren't supported yet");
                }
                let key = name.to_lowercase();
                if let Some((_, first)) = seen.iter().find(|(k, _)| *k == key) {
                    if *first == installer {
                        bail!("{name} is listed twice under install.{installer}");
                    }
                    bail!(
                        "{name} is listed under both {first} and {installer}; pick one installer"
                    );
                }
                seen.push((key, installer));
            }
        }
        Ok(())
    }
}

fn suggest(key: &str) -> Option<&'static str> {
    match key {
        "apps" | "app" | "casks" => Some("cask"),
        "brews" | "formula" | "formulae" | "formulas" => Some("brew"),
        "nixpkgs" | "packages" | "tools" | "tool" => Some("nix"),
        _ => None,
    }
}

fn check_install_keys(table: &toml::Table) -> Result<()> {
    for key in table.keys() {
        if INSTALLERS.contains(&key.as_str()) {
            continue;
        }
        if key == "mas" {
            bail!("[install] mas: Mac App Store apps aren't supported yet");
        }
        match suggest(key) {
            Some(right) => bail!("[install] has no {key:?}; did you mean {right:?}?"),
            None => bail!("[install] has no {key:?}; it takes brew, cask and nix"),
        }
    }
    Ok(())
}

fn upgrade_tools_section(table: &mut toml::Table) -> Result<bool> {
    let Some(tools) = table.remove("tools") else {
        return Ok(false);
    };
    if table.contains_key("install") {
        bail!(
            "there is both an old [tools] and an [install] section; move everything into [install]"
        );
    }
    let toml::Value::Table(mut tools) = tools else {
        bail!("tools must be a section");
    };
    if let Some(apps) = tools.remove("apps") {
        tools.insert("cask".into(), apps);
    }
    table.insert("install".into(), toml::Value::Table(tools));
    Ok(true)
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
        Config::parse(&text).with_context(|| format!("in {}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Config> {
        let mut table: toml::Table = toml::from_str(text)?;
        let old_tools_section = upgrade_tools_section(&mut table)?;
        if let Some(install) = table.get("install") {
            let toml::Value::Table(install) = install else {
                bail!("install must be a section");
            };
            check_install_keys(install)?;
        }
        let mut config: Config = table.try_into()?;
        config.old_tools_section = old_tools_section;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        self.install.validate()?;
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
            "{} is ready for backups; your tools and apps, backup rules and restore presets go in {}",
            drive.display(),
            path.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
