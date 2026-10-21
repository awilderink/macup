use crate::selection::at_or_under;
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
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InstallConfig {
    pub brew: Vec<String>,
    pub cask: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Brew,
    Cask,
}

impl Kind {
    pub const ALL: [Kind; 2] = [Kind::Brew, Kind::Cask];

    pub fn key(self) -> &'static str {
        match self {
            Kind::Brew => "brew",
            Kind::Cask => "cask",
        }
    }

    pub fn api(self) -> &'static str {
        match self {
            Kind::Brew => "formula",
            Kind::Cask => "cask",
        }
    }

    pub fn brew_flag(self) -> &'static str {
        match self {
            Kind::Brew => "--formula",
            Kind::Cask => "--cask",
        }
    }

    pub fn flag(self) -> &'static str {
        match self {
            Kind::Brew => "--brew",
            Kind::Cask => "--cask",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Brew => "Homebrew formula",
            Kind::Cask => "Homebrew cask",
        }
    }
}

impl InstallConfig {
    pub fn lists(&self) -> [(Kind, &Vec<String>); 2] {
        [(Kind::Brew, &self.brew), (Kind::Cask, &self.cask)]
    }

    pub fn is_empty(&self) -> bool {
        self.lists().iter().all(|(_, list)| list.is_empty())
    }

    pub fn installer_of(&self, name: &str) -> Option<Kind> {
        let name = name.to_lowercase();
        self.lists()
            .into_iter()
            .find(|(_, list)| list.iter().any(|n| n.to_lowercase() == name))
            .map(|(kind, _)| kind)
    }

    fn validate(&self) -> Result<()> {
        let mut seen: Vec<(String, Kind)> = Vec::new();
        for (kind, list) in self.lists() {
            let installer = kind.key();
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
                    if *first == kind {
                        bail!("{name} is listed twice under install.{installer}");
                    }
                    bail!(
                        "{name} is listed under both {} and {installer}; pick one installer",
                        first.key()
                    );
                }
                seen.push((key, kind));
            }
        }
        Ok(())
    }
}

fn suggest(key: &str) -> Option<&'static str> {
    match key {
        "apps" | "app" | "casks" => Some("cask"),
        "brews" | "formula" | "formulae" | "formulas" | "tools" => Some("brew"),
        _ => None,
    }
}

fn check_install_keys(table: &toml::Table) -> Result<()> {
    for key in table.keys() {
        if Kind::ALL.iter().any(|k| k.key() == key) {
            continue;
        }
        if key == "mas" {
            bail!("[install] mas: Mac App Store apps aren't supported yet");
        }
        match suggest(key) {
            Some(right) => bail!("[install] has no {key:?}; did you mean {right:?}?"),
            None => bail!("[install] has no {key:?}; it takes brew and cask"),
        }
    }
    Ok(())
}

pub const COMES_WITH_MACOS: [&str; 2] = ["curl", "git"];

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BackupConfig {
    pub skip: Vec<String>,
    pub include: Vec<String>,
}

pub fn convert_rules(rules: &[String]) -> Result<(Vec<String>, Vec<String>)> {
    let (mut skip, mut include): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    for rule in rules {
        let (sign, pattern) = match (rule.strip_prefix("- "), rule.strip_prefix("+ ")) {
            (Some(p), _) => ('-', p),
            (_, Some(p)) => ('+', p),
            _ => bail!(
                "backup rule {rule:?} must start with \"+ \" or \"- \"; better, move it under skip or include"
            ),
        };
        let base = pattern.strip_suffix("/***").unwrap_or(pattern);
        if sign == '+' {
            if !base.starts_with('/') {
                bail!(
                    "backup rule {rule:?} can't be converted; write it under include as a path from your home folder"
                );
            }
            let base = base.trim_end_matches('/').to_string();
            if !include.contains(&base) {
                include.push(base);
            }
            continue;
        }
        let inside_an_include = include
            .iter()
            .find(|k| !base.starts_with('/') || at_or_under(base.trim_end_matches('/'), k));
        if let Some(k) = inside_an_include {
            bail!(
                "backup rule {rule:?} comes after \"+ {k}\" and could skip things inside it; with skip and include, skips always win, so move these two to skip and include by hand"
            );
        }
        let base = base.to_string();
        if !skip.contains(&base) {
            skip.push(base);
        }
    }
    Ok((skip, include))
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
        let upgraded = crate::tools::upgrade(text)?;
        let table: toml::Table = toml::from_str(upgraded.as_deref().unwrap_or(text))?;
        if let Some(install) = table.get("install") {
            let toml::Value::Table(install) = install else {
                bail!("install must be a section");
            };
            check_install_keys(install)?;
        }
        let config: Config = table.try_into()?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        self.install.validate()?;
        for skip in &self.backup.skip {
            crate::pattern::Pattern::parse(skip)
                .with_context(|| format!("backup.skip {skip:?}"))?;
        }
        for include in &self.backup.include {
            crate::rules::check_include(include)?;
        }
        Ok(())
    }
}

pub const TEMPLATE: &str = "[backup]\nskip = []\ninclude = []\n\n[restore]\npresets = []\n";

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
