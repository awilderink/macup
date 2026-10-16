use crate::env::Env;
use crate::nix::{ensure_config, find_nix, nix_args, offer_apply};
use crate::ui::{note, ok};
use anyhow::Context;
use anyhow::{Result, bail};
use rnix::{Root, SyntaxKind, SyntaxNode};
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct ListSpec {
    pub path: &'static [&'static str],
    pub quoted: bool,
    pub describe: &'static str,
}

pub const PACKAGES: ListSpec = ListSpec {
    path: &["home", "packages"],
    quoted: false,
    describe: "home.packages",
};
pub const CASKS: ListSpec = ListSpec {
    path: &["homebrew", "casks"],
    quoted: true,
    describe: "homebrew.casks",
};

fn attr_names(attrpath: &SyntaxNode) -> Vec<String> {
    attrpath
        .children()
        .map(|a| a.text().to_string().trim_matches('"').to_string())
        .collect()
}

fn local_path(binding: &SyntaxNode) -> Vec<String> {
    let mut path = binding
        .children()
        .find(|c| c.kind() == SyntaxKind::NODE_ATTRPATH)
        .map(|a| attr_names(&a))
        .unwrap_or_default();
    let mut set = binding.parent();
    while let Some(s) = set.filter(|s| s.kind() == SyntaxKind::NODE_ATTR_SET) {
        let Some(outer) = s
            .parent()
            .filter(|p| p.kind() == SyntaxKind::NODE_ATTRPATH_VALUE)
        else {
            break;
        };
        let mut prefix = outer
            .children()
            .find(|c| c.kind() == SyntaxKind::NODE_ATTRPATH)
            .map(|a| attr_names(&a))
            .unwrap_or_default();
        prefix.extend(path);
        path = prefix;
        set = outer.parent();
    }
    path
}

fn binding_value(binding: &SyntaxNode) -> Option<SyntaxNode> {
    binding
        .children()
        .filter(|c| c.kind() != SyntaxKind::NODE_ATTRPATH)
        .last()
}

struct Found {
    list: SyntaxNode,
    with_namespace: bool,
}

fn list_in(value: SyntaxNode, with_namespace: bool) -> Option<Found> {
    match value.kind() {
        SyntaxKind::NODE_LIST => Some(Found {
            list: value,
            with_namespace,
        }),
        SyntaxKind::NODE_WITH => list_in(value.children().last()?, true),
        SyntaxKind::NODE_BIN_OP | SyntaxKind::NODE_PAREN => {
            list_in(value.first_child()?, with_namespace)
        }
        _ => None,
    }
}

fn find_list(root: &SyntaxNode, spec: &ListSpec) -> Vec<Found> {
    let target: Vec<String> = spec.path.iter().map(|p| p.to_string()).collect();
    root.descendants()
        .filter(|n| n.kind() == SyntaxKind::NODE_ATTRPATH_VALUE)
        .filter(|n| local_path(n).ends_with(&target))
        .filter_map(|n| list_in(binding_value(&n)?, false))
        .collect()
}

fn offset(n: rnix::TextSize) -> usize {
    u32::from(n) as usize
}

fn item_end(item: &SyntaxNode) -> usize {
    let mut end = offset(item.text_range().end());
    let mut next = item.next_sibling_or_token();
    while let Some(el) = next {
        match el.kind() {
            SyntaxKind::TOKEN_WHITESPACE if !el.to_string().contains('\n') => {}
            SyntaxKind::TOKEN_COMMENT => end = offset(el.text_range().end()),
            _ => break,
        }
        next = el.next_sibling_or_token();
    }
    end
}

fn item_name(item: &SyntaxNode) -> String {
    let text = item.text().to_string();
    let text = text.trim_matches('"');
    text.strip_prefix("pkgs.").unwrap_or(text).to_string()
}

fn edit_once(src: &str, spec: &ListSpec, name: &str, add: bool) -> Result<Option<String>> {
    let parsed = Root::parse(src);
    if let Some(err) = parsed.errors().first() {
        bail!("cannot parse this file as Nix: {err}");
    }
    let root = parsed.syntax();
    let found = find_list(&root, spec);
    let Some(found) = found.first() else {
        return Ok(None);
    };
    let items: Vec<SyntaxNode> = found.list.children().collect();
    let existing = items.iter().position(|i| item_name(i) == name);
    let mut out = src.to_string();
    if !add {
        let Some(i) = existing else {
            return Ok(Some(out));
        };
        let item = &items[i];
        let start = item
            .prev_sibling_or_token()
            .filter(|t| t.kind() == SyntaxKind::TOKEN_WHITESPACE)
            .map(|t| offset(t.text_range().start()))
            .unwrap_or(offset(item.text_range().start()));
        out.replace_range(start..item_end(item), "");
        return Ok(Some(out));
    }
    if existing.is_some() {
        return Ok(Some(out));
    }
    let prefixed = !spec.quoted
        && (!found.with_namespace
            || items
                .iter()
                .any(|i| i.text().to_string().starts_with("pkgs.")));
    let text = match (spec.quoted, prefixed) {
        (true, _) => format!("\"{name}\""),
        (false, true) => format!("pkgs.{name}"),
        (false, false) => name.to_string(),
    };
    let Some(first) = items.first() else {
        let open = found
            .list
            .first_token()
            .map(|t| offset(t.text_range().end()))
            .unwrap_or(offset(found.list.text_range().start()) + 1);
        let close = offset(found.list.text_range().end()) - 1;
        out.replace_range(open..close, &format!(" {text} "));
        return Ok(Some(out));
    };
    let separator = first
        .prev_sibling_or_token()
        .filter(|t| t.kind() == SyntaxKind::TOKEN_WHITESPACE)
        .map(|t| t.to_string())
        .filter(|w| w.contains('\n'))
        .map(|w| format!("\n{}", w.rsplit('\n').next().unwrap_or("")))
        .unwrap_or_else(|| " ".to_string());
    let key = name.to_lowercase();
    match items
        .iter()
        .rposition(|i| item_name(i).to_lowercase() < key)
    {
        Some(i) => out.insert_str(item_end(&items[i]), &format!("{separator}{text}")),
        None => out.insert_str(
            offset(first.text_range().start()),
            &format!("{text}{separator}"),
        ),
    }
    Ok(Some(out))
}

pub fn edit_list(
    src: &str,
    spec: &ListSpec,
    names: &[String],
    add: bool,
) -> Result<Option<(String, Vec<String>)>> {
    let parsed = Root::parse(src);
    let root = parsed.syntax();
    let found = find_list(&root, spec);
    if found.len() > 1 {
        bail!(
            "{} appears {} times in this file; edit it by hand with `macup edit`",
            spec.describe,
            found.len()
        );
    }
    let Some(found) = found.first() else {
        return Ok(None);
    };
    let present: Vec<String> = found.list.children().map(|i| item_name(&i)).collect();
    let mut out = src.to_string();
    let mut changed = Vec::new();
    for name in names {
        if present.contains(name) == add {
            continue;
        }
        out = edit_once(&out, spec, name, add)?.expect("the list was found above");
        changed.push(name.clone());
    }
    Ok(Some((out, changed)))
}

fn nix_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if name.ends_with(".nix") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

pub fn change(e: &Env, names: &[String], app: bool, add: bool) -> Result<()> {
    crate::nix::require_nix(e)?;
    if names.is_empty() {
        bail!(
            "usage: macup {} [--app] <name>...",
            if add { "add" } else { "remove" }
        );
    }
    ensure_config(e)?;
    let spec = if app { &CASKS } else { &PACKAGES };
    let mut hits = Vec::new();
    for file in nix_files(&e.nix_dir()) {
        let src = std::fs::read_to_string(&file)?;
        if let Some(edit) =
            edit_list(&src, spec, names, add).with_context(|| format!("in {}", file.display()))?
        {
            hits.push((file, edit));
        }
    }
    let (file, (out, changed)) = match hits.len() {
        0 => bail!(
            "could not find a {} list in {}; edit it by hand with `macup edit`",
            spec.describe,
            e.nix_dir().display()
        ),
        1 => hits.remove(0),
        _ => bail!(
            "{} is defined in several files ({}); edit it by hand with `macup edit`",
            spec.describe,
            hits.iter()
                .map(|(f, _)| f.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    if add {
        for n in names {
            validate(e, n, app)?;
        }
    }
    if changed.is_empty() {
        ok("nothing to change");
        return Ok(());
    }
    std::fs::write(&file, out)?;
    let shown = file
        .strip_prefix(e.nix_dir())
        .unwrap_or(&file)
        .display()
        .to_string();
    ok(format!(
        "{} {} in {shown}",
        if add { "Added" } else { "Removed" },
        changed.join(", ")
    ));
    offer_apply(e)
}

fn validate(e: &Env, name: &str, app: bool) -> Result<()> {
    if app {
        let Some(brew) = crate::rsync::which("brew") else {
            return Ok(());
        };
        if !Command::new(brew)
            .args(["info", "--cask", name])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            bail!("Homebrew has no app called {name:?} (try `brew search {name}`)");
        }
        return Ok(());
    }
    let Some(nix) = find_nix() else {
        bail!("Nix is not installed yet — run `macup apply` first")
    };
    let dir = e.nix_dir().display().to_string();
    let attr = format!("nixpkgs#{name}.name");
    let out = Command::new(nix)
        .args(nix_args(&["eval", "--raw", "--inputs-from", &dir, &attr]))
        .output()?;
    if !out.status.success() {
        bail!(
            "nixpkgs has no package called {name:?} (search at https://search.nixos.org/packages?query={name})"
        );
    }
    note(format!(
        "found {}",
        String::from_utf8_lossy(&out.stdout).trim()
    ));
    Ok(())
}

#[cfg(test)]
#[path = "packages_tests.rs"]
mod tests;
