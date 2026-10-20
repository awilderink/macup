use crate::config::{COMES_WITH_MACOS, convert_rules};
use anyhow::{Context, Result, anyhow, bail};
use toml_edit::{Array, DocumentMut, Item, Key, Table, TableLike, Value};

fn parse_doc(src: &str) -> Result<DocumentMut> {
    src.parse().context("macup.toml is not valid TOML")
}

fn install_table(doc: &mut DocumentMut) -> Result<&mut dyn TableLike> {
    doc.entry("install")
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_like_mut()
        .ok_or_else(|| anyhow!("install in macup.toml must be a section"))
}

pub fn upgrade(src: &str) -> Result<Option<String>> {
    let mut doc = parse_doc(src)?;
    let mut changed = doc.remove("nix").is_some();
    if let Some(backup) = doc.get_mut("backup").and_then(Item::as_table_like_mut)
        && let Some(rules) = backup.remove("rules")
    {
        let Some(rules) = rules.as_array() else {
            bail!("backup.rules must be a list");
        };
        let rules: Vec<String> = rules
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        let (skip, keep) = convert_rules(&rules)?;
        backup.insert("skip", Item::Value(Value::Array(lines(&skip))));
        backup.insert("keep", Item::Value(Value::Array(lines(&keep))));
        changed = true;
    }
    if let Some(mut tools) = doc.remove("tools") {
        if doc.contains_key("install") {
            bail!(
                "there is both an old [tools] and an [install] section; move everything into [install]"
            );
        }
        if let Some(table) = tools.as_table_mut() {
            if let Some((key, apps)) = table.remove_entry("apps") {
                let cask = Key::new("cask").with_leaf_decor(key.leaf_decor().clone());
                table.insert_formatted(&cask, apps);
            }
        } else if let Some(table) = tools.as_table_like_mut() {
            if let Some(apps) = table.remove("apps") {
                table.insert("cask", apps);
            }
        } else {
            bail!("tools must be a section");
        }
        doc.insert("install", tools);
        changed = true;
    }
    if let Some(install) = doc.get_mut("install").and_then(Item::as_table_like_mut)
        && let Some(nix) = install.remove("nix")
    {
        let Some(nix) = nix.as_array() else {
            bail!("install.nix must be a list");
        };
        for name in nix.iter().filter_map(|v| v.as_str()) {
            match name {
                "rustup" => {
                    install.insert("rust", toml_edit::value(true));
                }
                name if COMES_WITH_MACOS.contains(&name) => {}
                name => bail!(
                    "install.nix lists {name}, but macup no longer installs from nixpkgs; move it to brew (`macup add --brew {name}`) or delete it"
                ),
            }
        }
        changed = true;
    }
    Ok(changed.then(|| doc.to_string()))
}

fn lines(items: &[String]) -> Array {
    let mut list = Array::new();
    for item in items {
        list.push(item.as_str());
    }
    for v in list.iter_mut() {
        v.decor_mut().set_prefix("\n  ");
    }
    if !items.is_empty() {
        list.set_trailing("\n");
        list.set_trailing_comma(true);
    }
    list
}

pub fn set_rust(src: &str) -> Result<String> {
    let mut doc = parse_doc(src)?;
    install_table(&mut doc)?.insert("rust", toml_edit::value(true));
    Ok(doc.to_string())
}

pub fn edit(src: &str, key: &str, names: &[String], add: bool) -> Result<(String, Vec<String>)> {
    let mut doc = parse_doc(src)?;
    let list = install_table(&mut doc)?
        .entry(key)
        .or_insert(Item::Value(Value::Array(Array::new())))
        .as_array_mut()
        .ok_or_else(|| anyhow!("install.{key} in macup.toml must be a list of names"))?;
    let mut changed = Vec::new();
    for name in names {
        match (add, position(list, name)) {
            (true, None) => insert_sorted(list, name),
            (false, Some(i)) => {
                list.remove(i);
            }
            _ => continue,
        }
        changed.push(name.clone());
    }
    Ok((doc.to_string(), changed))
}

fn position(list: &Array, name: &str) -> Option<usize> {
    list.iter().position(|v| v.as_str() == Some(name))
}

fn insert_sorted(list: &mut Array, name: &str) {
    let key = name.to_lowercase();
    let at = list
        .iter()
        .position(|v| v.as_str().is_some_and(|s| s.to_lowercase() > key))
        .unwrap_or(list.len());
    let indent = list
        .get(at.min(list.len().saturating_sub(1)))
        .and_then(|v| v.decor().prefix())
        .and_then(|p| p.as_str())
        .and_then(|p| p.rfind('\n').map(|i| p[i..].to_string()));
    list.insert(at, name);
    match indent {
        Some(indent) => {
            if let Some(v) = list.get_mut(at) {
                v.decor_mut().set_prefix(indent);
            }
        }
        None => list.fmt(),
    }
}

#[cfg(test)]
#[path = "tools_tests.rs"]
mod tests;
