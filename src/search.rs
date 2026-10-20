use serde_json::Value;
use std::path::Path;
use std::process::Command;

const SHOWN: usize = 8;
const BREW_API: &str = "https://formulae.brew.sh/api";

#[derive(Debug, PartialEq, Eq)]
pub enum Lookup {
    Found { name: String, detail: String },
    Missing,
    Unknown(String),
}

fn brew_item(info: &Value) -> &Value {
    ["casks", "formulae"]
        .into_iter()
        .find_map(|key| info.get(key).and_then(|list| list.get(0)))
        .unwrap_or(info)
}

pub fn brew_found(asked: &str, info_json: &[u8]) -> Lookup {
    let info: Value = serde_json::from_slice(info_json).unwrap_or_default();
    let name = ["token", "name"]
        .into_iter()
        .find_map(|key| brew_item(&info).get(key).and_then(Value::as_str))
        .unwrap_or(asked)
        .to_string();
    Lookup::Found {
        name,
        detail: brew_version(info_json).unwrap_or_default(),
    }
}

pub fn brew_version(info_json: &[u8]) -> Option<String> {
    let info: Value = serde_json::from_slice(info_json).ok()?;
    let item = brew_item(&info);
    let version = item
        .get("version")
        .or_else(|| item.pointer("/versions/stable"))?
        .as_str()?;
    Some(version.split(',').next().unwrap_or(version).to_string())
}

pub fn brew_from_api(kind: &str, name: &str) -> Lookup {
    let out = Command::new("/usr/bin/curl")
        .args(["-s", "-m", "5", "-w", "\n%{http_code}"])
        .arg(format!("{BREW_API}/{kind}/{name}.json"))
        .output();
    let Ok(out) = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()) else {
        return Lookup::Unknown("curl is missing".into());
    };
    match out.rsplit_once('\n') {
        Some((body, "200")) => brew_found(name, body.as_bytes()),
        Some((_, "404")) => Lookup::Missing,
        _ => Lookup::Unknown("formulae.brew.sh did not answer".into()),
    }
}

pub fn similar_brews(home: &Path, kind: &str, name: &str) -> Vec<String> {
    std::fs::read_to_string(home.join(format!("Library/Caches/Homebrew/api/{kind}_names.txt")))
        .map(|names| similar(name, names.lines()))
        .unwrap_or_default()
}

pub fn similar<'a>(query: &str, names: impl Iterator<Item = &'a str>) -> Vec<String> {
    let query = query.to_lowercase();
    let limit = (query.chars().count() / 3).max(2);
    let mut scored: Vec<(usize, &str)> = names
        .filter(|n| !n.is_empty() && *n != query)
        .filter_map(|n| {
            let distance = levenshtein(&query, n);
            let score = if n.contains(&query) {
                n.len() - query.len()
            } else if distance <= limit {
                distance
            } else {
                return None;
            };
            Some((score, n))
        })
        .collect();
    scored.sort();
    scored
        .into_iter()
        .take(SHOWN / 2)
        .map(|(_, n)| n.to_string())
        .collect()
}

fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let next = (row[j + 1] + 1)
                .min(row[j] + 1)
                .min(prev + usize::from(ca != *cb));
            prev = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
