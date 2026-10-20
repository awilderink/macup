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
    #[serde(skip)]
    pub old_layout: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InstallConfig {
    pub brew: Vec<String>,
    pub cask: Vec<String>,
    pub rust: bool,
}

pub const LISTS: [&str; 2] = ["brew", "cask"];

impl InstallConfig {
    pub fn lists(&self) -> [(&'static str, &Vec<String>); 2] {
        [("brew", &self.brew), ("cask", &self.cask)]
    }

    pub fn is_empty(&self) -> bool {
        !self.rust && self.lists().iter().all(|(_, list)| list.is_empty())
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
                if name.contains('/') {
                    bail!("{name}: Homebrew taps other than Homebrew's own aren't supported yet");
                }
                if name.is_empty()
                    || !name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "._+@-".contains(c))
                {
                    bail!(
                        "install.{installer} has {name:?}, which is not a Homebrew name (letters, digits and . _ + @ - only)"
                    );
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
        "brews" | "formula" | "formulae" | "formulas" | "tools" => Some("brew"),
        "rustup" => Some("rust"),
        _ => None,
    }
}

fn check_install_keys(table: &toml::Table) -> Result<()> {
    for key in table.keys() {
        if LISTS.contains(&key.as_str()) || key == "rust" {
            continue;
        }
        if key == "mas" {
            bail!("[install] mas: Mac App Store apps aren't supported yet");
        }
        match suggest(key) {
            Some(right) => bail!("[install] has no {key:?}; did you mean {right:?}?"),
            None => bail!("[install] has no {key:?}; it takes brew, cask and rust"),
        }
    }
    Ok(())
}

pub const COMES_WITH_MACOS: [&str; 2] = ["curl", "git"];

fn upgrade_old_layout(table: &mut toml::Table) -> Result<bool> {
    let mut changed = table.remove("nix").is_some();
    if let Some(tools) = table.remove("tools") {
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
        changed = true;
    }
    let Some(toml::Value::Table(install)) = table.get_mut("install") else {
        return Ok(changed);
    };
    let Some(nix) = install.remove("nix") else {
        return Ok(changed);
    };
    let toml::Value::Array(nix) = nix else {
        bail!("install.nix must be a list");
    };
    for name in nix.iter().filter_map(|v| v.as_str()) {
        match name {
            "rustup" => {
                install.insert("rust".into(), toml::Value::Boolean(true));
            }
            name if COMES_WITH_MACOS.contains(&name) => {}
            name => bail!(
                "install.nix lists {name}, but macup no longer installs from nixpkgs; move it to brew (`macup add --brew {name}`) or delete it"
            ),
        }
    }
    Ok(true)
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
        let old_layout = upgrade_old_layout(&mut table)?;
        if let Some(install) = table.get("install") {
            let toml::Value::Table(install) = install else {
                bail!("install must be a section");
            };
            check_install_keys(install)?;
        }
        let mut config: Config = table.try_into()?;
        config.old_layout = old_layout;
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
