use crate::config::Config;
use crate::pattern::Pattern;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::LazyLock;

const BUILTIN: &str = include_str!("../rules/builtin.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Builtin {
    skip: Vec<String>,
    never_skip: Vec<String>,
    left_out: Vec<String>,
    only: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Default)]
pub struct Filter {
    never_skip: Vec<Pattern>,
    skip: Vec<Pattern>,
    left_out: Vec<Pattern>,
    only: Vec<String>,
    include: Vec<Pattern>,
}

fn parse_all(texts: &[String]) -> Result<Vec<Pattern>> {
    texts.iter().map(|t| Pattern::parse(t)).collect()
}

fn builtin() -> Result<Filter> {
    let b: Builtin = toml::from_str(BUILTIN).context("reading the built-in rules")?;
    let mut include = Vec::new();
    let mut only = Vec::new();
    for (folder, entries) in &b.only {
        let folder = folder.trim_matches('/').to_string();
        for entry in entries {
            include.push(Pattern::parse(&format!("/{folder}/{entry}"))?);
        }
        only.push(folder);
    }
    Ok(Filter {
        never_skip: parse_all(&b.never_skip)?,
        skip: parse_all(&b.skip)?,
        left_out: parse_all(&b.left_out)?,
        only,
        include,
    })
}

static BUILTIN_FILTER: LazyLock<Filter> =
    LazyLock::new(|| builtin().expect("the built-in rules are valid"));

impl Filter {
    pub fn skipping(patterns: &[String]) -> Result<Filter> {
        Ok(Filter {
            skip: parse_all(patterns)?,
            ..Filter::default()
        })
    }

    pub fn backup(config: &Config, extra_skips: &[String]) -> Result<Filter> {
        let mut f = BUILTIN_FILTER.clone();
        f.skip.extend(parse_all(&config.backup.skip)?);
        f.skip.extend(parse_all(extra_skips)?);
        f.include.extend(parse_all(&config.backup.include)?);
        Ok(f)
    }

    pub fn copies(&self, rel: &str, is_dir: bool) -> bool {
        if self.never_skip.iter().any(|p| p.matches(rel, is_dir)) {
            return true;
        }
        if self.skip.iter().any(|p| p.matches(rel, is_dir)) {
            return false;
        }
        if !self.left_out_by_default(rel, is_dir) {
            return true;
        }
        if self.include.iter().any(|p| p.covers(rel, is_dir)) {
            return true;
        }
        is_dir && self.include.iter().any(|p| p.leads_into(rel))
    }

    fn left_out_by_default(&self, rel: &str, is_dir: bool) -> bool {
        self.left_out.iter().any(|p| p.covers(rel, is_dir))
            || self.only.iter().any(|folder| {
                rel.strip_prefix(folder.as_str())
                    .is_some_and(|rest| rest.starts_with('/'))
            })
    }
}

pub fn check_include(text: &str) -> Result<()> {
    let pattern = Pattern::parse(text)?;
    if !pattern.anchored() {
        bail!("include {text:?} must be a path from your home folder, starting with /");
    }
    let rel = text.trim_matches('/');
    if !BUILTIN_FILTER.left_out_by_default(rel, true) {
        bail!(
            "include {text:?} is backed up anyway; include only adds back what macup leaves out by default (~/Library outside Application Support, most of ~/.claude, and caches like ~/.npm and ~/.rustup)"
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "rules_tests.rs"]
mod tests;
