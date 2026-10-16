use anyhow::Result;
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckState {
    Unchecked,
    Partial,
    Checked,
}

pub struct Selection {
    pub paths: HashSet<String>,
    root: PathBuf,
    cache: RefCell<HashMap<String, Vec<String>>>,
}

pub fn join_rel(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

pub fn parent_rel(rel: &str) -> &str {
    rel.rsplit_once('/').map(|(p, _)| p).unwrap_or("")
}

pub fn is_under(p: &str, dir: &str) -> bool {
    if dir.is_empty() {
        !p.is_empty()
    } else {
        p.len() > dir.len() + 1 && p.starts_with(dir) && p.as_bytes()[dir.len()] == b'/'
    }
}

pub fn existing_paths(root: &Path, rels: &[String]) -> Vec<String> {
    rels.iter()
        .filter(|r| std::fs::symlink_metadata(root.join(r)).is_ok())
        .cloned()
        .collect()
}

impl Selection {
    pub fn new(root: &Path) -> Selection {
        Selection {
            paths: HashSet::new(),
            root: root.to_path_buf(),
            cache: RefCell::new(HashMap::new()),
        }
    }

    fn list(&self, rel: &str) -> Result<Vec<String>> {
        if let Some(names) = self.cache.borrow().get(rel) {
            return Ok(names.clone());
        }
        let mut names = Vec::new();
        for entry in std::fs::read_dir(self.root.join(rel))? {
            names.push(join_rel(rel, &entry?.file_name().to_string_lossy()));
        }
        self.cache
            .borrow_mut()
            .insert(rel.to_string(), names.clone());
        Ok(names)
    }

    fn covering_ancestor(&self, rel: &str) -> Option<String> {
        let mut p = rel;
        while !p.is_empty() {
            if self.paths.contains(p) {
                return Some(p.to_string());
            }
            p = parent_rel(p);
        }
        None
    }

    pub fn state(&self, rel: &str) -> CheckState {
        if self.covering_ancestor(rel).is_some() {
            CheckState::Checked
        } else if self.paths.iter().any(|p| is_under(p, rel)) {
            CheckState::Partial
        } else {
            CheckState::Unchecked
        }
    }

    pub fn toggle(&mut self, rel: &str) -> Result<()> {
        if self.covering_ancestor(rel).is_some() {
            self.remove(rel)
        } else {
            self.add(rel)
        }
    }

    pub fn add(&mut self, rel: &str) -> Result<()> {
        if self.covering_ancestor(rel).is_some() {
            return Ok(());
        }
        self.paths.retain(|p| !is_under(p, rel));
        self.paths.insert(rel.to_string());
        self.collapse(parent_rel(rel).to_string())
    }

    fn collapse(&mut self, dir: String) -> Result<()> {
        if dir.is_empty() {
            return Ok(());
        }
        let children = self.list(&dir)?;
        if children.is_empty() || !children.iter().all(|c| self.paths.contains(c)) {
            return Ok(());
        }
        for c in &children {
            self.paths.remove(c);
        }
        let parent = parent_rel(&dir).to_string();
        self.paths.insert(dir);
        self.collapse(parent)
    }

    pub fn remove(&mut self, rel: &str) -> Result<()> {
        let Some(anc) = self.covering_ancestor(rel) else {
            return Ok(());
        };
        self.paths.remove(&anc);
        let mut cur = anc;
        while cur != rel {
            let rest = &rel[cur.len() + 1..];
            let next = join_rel(&cur, rest.split('/').next().unwrap_or(rest));
            for c in self.list(&cur)? {
                if c != next {
                    self.paths.insert(c);
                }
            }
            cur = next;
        }
        Ok(())
    }

    pub fn clear(&mut self) {
        self.paths.clear();
    }

    pub fn sorted(&self) -> Vec<String> {
        self.paths
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
}

#[cfg(test)]
#[path = "selection_tests.rs"]
pub mod tests;
