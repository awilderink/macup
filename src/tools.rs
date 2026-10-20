use anyhow::{Context, Result, anyhow};
use toml_edit::{Array, DocumentMut, Item, Key, Table, Value};

pub fn upgrade(src: &str) -> Result<Option<String>> {
    let mut doc: DocumentMut = src.parse().context("macup.toml is not valid TOML")?;
    if doc.contains_key("install") {
        return Ok(None);
    }
    let Some(mut tools) = doc.remove("tools") else {
        return Ok(None);
    };
    if let Some(table) = tools.as_table_mut() {
        if let Some((key, apps)) = table.remove_entry("apps") {
            let cask = Key::new("cask").with_leaf_decor(key.leaf_decor().clone());
            table.insert_formatted(&cask, apps);
        }
    } else if let Some(table) = tools.as_table_like_mut()
        && let Some(apps) = table.remove("apps")
    {
        table.insert("cask", apps);
    }
    doc.insert("install", tools);
    Ok(Some(doc.to_string()))
}

pub fn edit(src: &str, key: &str, names: &[String], add: bool) -> Result<(String, Vec<String>)> {
    let mut doc: DocumentMut = src.parse().context("macup.toml is not valid TOML")?;
    let install = doc
        .entry("install")
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_like_mut()
        .ok_or_else(|| anyhow!("install in macup.toml must be a section"))?;
    let list = install
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
