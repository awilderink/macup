use serde::Deserialize;
use serde_json::{Value, json};
use std::process::Command;

const BACKEND: &str = "https://search.nixos.org/backend";
const PUBLIC_FRONTEND_LOGIN: &str = "aWVSALXpZv:X8gPHnzL52wFEekuxsfQ9cSh";
const SHOWN: usize = 8;

#[derive(Debug, PartialEq)]
pub struct Hit {
    pub name: String,
    pub version: String,
    pub description: String,
}

#[derive(Deserialize)]
struct Response {
    #[serde(rename = "_shards")]
    shards: Shards,
    hits: Hits,
}

#[derive(Deserialize)]
struct Shards {
    total: u64,
}

#[derive(Deserialize)]
struct Hits {
    hits: Vec<Doc>,
}

#[derive(Deserialize)]
struct Doc {
    #[serde(rename = "_source")]
    source: Source,
}

#[derive(Deserialize)]
struct Source {
    package_attr_name: String,
    #[serde(default)]
    package_pversion: Option<String>,
    #[serde(default)]
    package_description: Option<String>,
}

pub fn web_url(name: &str) -> String {
    format!("https://search.nixos.org/packages?query={name}")
}

pub fn packages(release: Option<&str>, name: &str) -> Option<Vec<Hit>> {
    let channels = release
        .map(|r| format!("nixos-{r}"))
        .into_iter()
        .chain(["nixos-unstable".to_string()]);
    for channel in channels {
        let response: Response = serde_json::from_str(&fetch(&channel, &query(name))?).ok()?;
        if response.shards.total > 0 {
            return Some(response.into_hits());
        }
    }
    None
}

fn query(name: &str) -> Value {
    let platform = format!("{}-darwin", std::env::consts::ARCH);
    json!({
        "size": SHOWN * 4,
        "_source": ["package_attr_name", "package_pversion", "package_description"],
        "query": { "bool": {
            "filter": [
                { "term": { "type": "package" } },
                { "term": { "package_platforms": platform } }
            ],
            "should": [
                { "term": { "package_attr_name": { "value": name, "boost": 50 } } },
                { "term": { "package_programs": { "value": name, "boost": 30 } } },
                { "multi_match": {
                    "query": name,
                    "fuzziness": "AUTO",
                    "fields": [
                        "package_attr_name.edge^9",
                        "package_pname.edge^6",
                        "package_programs.edge^6",
                        "package_attr_name.attr_path^4"
                    ]
                } },
                { "wildcard": { "package_attr_name": { "value": format!("*{name}*"), "boost": 3 } } },
                { "match": { "package_description": { "query": name, "boost": 0.5 } } },
                { "term": { "package_attr_set": { "value": "No package set", "boost": 2 } } }
            ],
            "minimum_should_match": 2
        } }
    })
}

fn fetch(channel: &str, query: &Value) -> Option<String> {
    let out = Command::new("/usr/bin/curl")
        .args(["-sf", "-m", "5", "-u", PUBLIC_FRONTEND_LOGIN])
        .args(["-H", "Content-Type: application/json", "-d"])
        .arg(query.to_string())
        .arg(format!("{BACKEND}/latest-*-{channel}/_search"))
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

impl Response {
    fn into_hits(self) -> Vec<Hit> {
        let mut hits: Vec<Hit> = Vec::new();
        for doc in self.hits.hits {
            let s = doc.source;
            if hits.iter().any(|h| h.name == s.package_attr_name) {
                continue;
            }
            hits.push(Hit {
                name: s.package_attr_name,
                version: s.package_pversion.unwrap_or_default(),
                description: s.package_description.unwrap_or_default(),
            });
            if hits.len() == SHOWN {
                break;
            }
        }
        hits
    }
}

pub fn format(hits: &[Hit]) -> String {
    let name_width = hits.iter().map(|h| h.name.len()).max().unwrap_or(0);
    let version_width = hits
        .iter()
        .map(|h| h.version.len())
        .max()
        .unwrap_or(0)
        .min(10);
    hits.iter()
        .map(|h| {
            let line = format!(
                "  {:name_width$}  {:version_width$}  {}",
                h.name,
                h.version,
                h.description.lines().next().unwrap_or("")
            );
            match line.char_indices().nth(100) {
                Some((cut, _)) => format!("{}…", &line[..cut]),
                None => line,
            }
            .trim_end()
            .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
