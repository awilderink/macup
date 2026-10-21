use anyhow::{Result, bail};

#[derive(Debug, Clone)]
pub struct Pattern {
    glob: String,
    anchored: bool,
    dir_only: bool,
    whole_path: bool,
    literal: bool,
}

impl Pattern {
    pub fn parse(text: &str) -> Result<Pattern> {
        if let Some(rest) = text.strip_prefix("- ").or_else(|| text.strip_prefix("+ ")) {
            bail!(
                "{text:?} looks like an old rsync rule; write {rest:?} under skip or include instead"
            );
        }
        if text.contains("***") {
            bail!(
                "{text:?}: *** is no longer needed; a folder that matches covers everything in it"
            );
        }
        let anchored = text.starts_with('/');
        let dir_only = text.len() > 1 && text.ends_with('/');
        let glob = text
            .trim_start_matches('/')
            .trim_end_matches('/')
            .to_string();
        if glob.is_empty() {
            bail!("{text:?} is not a pattern");
        }
        Ok(Pattern {
            whole_path: glob.contains('/') || glob.contains("**"),
            literal: !glob.contains(['*', '?', '[', '\\']),
            glob,
            anchored,
            dir_only,
        })
    }

    pub fn anchored(&self) -> bool {
        self.anchored
    }

    pub fn matches(&self, rel: &str, is_dir: bool) -> bool {
        if self.dir_only && !is_dir {
            return false;
        }
        let matches = |text: &str| {
            if self.literal {
                text == self.glob
            } else {
                glob_match(&self.glob, text)
            }
        };
        if self.anchored {
            return matches(rel);
        }
        if !self.whole_path {
            return matches(rel.rsplit('/').next().unwrap_or(rel));
        }
        tails(rel).any(matches)
    }

    pub fn covers(&self, rel: &str, is_dir: bool) -> bool {
        self.matches(rel, is_dir) || ancestors(rel).any(|a| self.matches(a, true))
    }

    pub fn leads_into(&self, rel: &str) -> bool {
        if !self.anchored {
            return false;
        }
        let mut want = self.glob.split('/');
        for have in rel.split('/') {
            match want.next() {
                Some("**") => return true,
                Some(segment) if glob_match(segment, have) => {}
                _ => return false,
            }
        }
        want.next().is_some()
    }
}

fn tails(rel: &str) -> impl Iterator<Item = &str> {
    std::iter::once(rel).chain(rel.match_indices('/').map(move |(i, _)| &rel[i + 1..]))
}

pub fn ancestors(rel: &str) -> impl Iterator<Item = &str> {
    rel.match_indices('/').map(move |(i, _)| &rel[..i])
}

pub fn glob_match(glob: &str, text: &str) -> bool {
    let g: Vec<char> = glob.chars().collect();
    let t: Vec<char> = text.chars().collect();
    matches_at(&g, &t)
}

fn matches_at(g: &[char], t: &[char]) -> bool {
    match g.first() {
        None => t.is_empty(),
        Some('*') if g.get(1) == Some(&'*') => {
            let rest = &g[2..];
            (0..=t.len()).any(|i| matches_at(rest, &t[i..]))
        }
        Some('*') => {
            let rest = &g[1..];
            for i in 0..=t.len() {
                if matches_at(rest, &t[i..]) {
                    return true;
                }
                if t.get(i) == Some(&'/') {
                    break;
                }
            }
            false
        }
        Some('?') => t.first().is_some_and(|c| *c != '/') && matches_at(&g[1..], &t[1..]),
        Some('[') => match class(g, t.first().copied()) {
            Some((true, len)) => matches_at(&g[len..], &t[1..]),
            Some((false, _)) => false,
            None => t.first() == Some(&'[') && matches_at(&g[1..], &t[1..]),
        },
        Some('\\') if g.len() > 1 => t.first() == Some(&g[1]) && matches_at(&g[2..], &t[1..]),
        Some(c) => t.first() == Some(c) && matches_at(&g[1..], &t[1..]),
    }
}

fn class(g: &[char], c: Option<char>) -> Option<(bool, usize)> {
    let mut i = 1;
    let negate = matches!(g.get(i), Some('!' | '^'));
    if negate {
        i += 1;
    }
    let start = i;
    let mut hit = false;
    while let Some(&ch) = g.get(i) {
        if ch == ']' && i > start {
            let ok = c.is_some_and(|c| c != '/') && hit != negate;
            return Some((ok, i + 1));
        }
        if g.get(i + 1) == Some(&'-') && g.get(i + 2).is_some_and(|e| *e != ']') {
            let (lo, hi) = (ch, g[i + 2]);
            hit |= c.is_some_and(|c| lo <= c && c <= hi);
            i += 3;
        } else {
            hit |= c == Some(ch);
            i += 1;
        }
    }
    None
}

#[cfg(test)]
#[path = "pattern_tests.rs"]
mod tests;
