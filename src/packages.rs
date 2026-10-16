use crate::env::Env;
use crate::nix::{ensure_config, find_nix, nix_args, offer_apply};
use crate::search;
use crate::ui::{confirm, interactive, note, ok, with_spinner};
use anyhow::{Context, Result, anyhow, bail};
use rnix::{Root, SyntaxKind, SyntaxNode, ast};
use rowan::ast::AstNode;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct ListSpec {
    pub path: &'static [&'static str],
    pub quoted: bool,
}

impl ListSpec {
    fn describe(&self) -> String {
        self.path.join(".")
    }
}

pub const PACKAGES: ListSpec = ListSpec {
    path: &["home", "packages"],
    quoted: false,
};
pub const CASKS: ListSpec = ListSpec {
    path: &["homebrew", "casks"],
    quoted: true,
};

fn unquote(node: &SyntaxNode) -> String {
    node.text().to_string().trim_matches('"').to_string()
}

fn item_name(item: &SyntaxNode) -> String {
    let name = unquote(item);
    name.strip_prefix("pkgs.")
        .map(str::to_string)
        .unwrap_or(name)
}

fn attr_names(binding: &ast::AttrpathValue) -> Vec<String> {
    binding
        .attrpath()
        .map(|p| p.attrs().map(|a| unquote(a.syntax())).collect())
        .unwrap_or_default()
}

fn local_path(binding: &ast::AttrpathValue) -> Vec<String> {
    let mut levels = vec![attr_names(binding)];
    let mut parent = binding.syntax().parent();
    while let Some(set) = parent.filter(|n| n.kind() == SyntaxKind::NODE_ATTR_SET) {
        let Some(outer) = set.parent().and_then(ast::AttrpathValue::cast) else {
            break;
        };
        levels.push(attr_names(&outer));
        parent = outer.syntax().parent();
    }
    levels.into_iter().rev().flatten().collect()
}

fn list_in(expr: ast::Expr) -> Option<ast::List> {
    match expr {
        ast::Expr::List(list) => Some(list),
        ast::Expr::With(with) => list_in(with.body()?),
        ast::Expr::BinOp(op) => list_in(op.lhs()?),
        ast::Expr::Paren(paren) => list_in(paren.expr()?),
        _ => None,
    }
}

fn locate(src: &str, spec: &ListSpec) -> Result<Option<ast::List>> {
    let root = Root::parse(src)
        .ok()
        .map_err(|err| anyhow!("cannot parse this file as Nix: {err}"))?;
    let mut lists: Vec<ast::List> = root
        .syntax()
        .descendants()
        .filter_map(ast::AttrpathValue::cast)
        .filter(|b| {
            local_path(b).ends_with(&spec.path.iter().map(|p| p.to_string()).collect::<Vec<_>>())
        })
        .filter_map(|b| list_in(b.value()?))
        .collect();
    if lists.len() > 1 {
        bail!(
            "{} appears {} times in this file; edit it by hand with `macup edit`",
            spec.describe(),
            lists.len()
        );
    }
    Ok(lists.pop())
}

fn item_end(item: &SyntaxNode) -> usize {
    let mut end = item.text_range().end();
    let mut next = item.next_sibling_or_token();
    while let Some(el) = next {
        match el.kind() {
            SyntaxKind::TOKEN_WHITESPACE if !el.to_string().contains('\n') => {}
            SyntaxKind::TOKEN_COMMENT => end = el.text_range().end(),
            _ => break,
        }
        next = el.next_sibling_or_token();
    }
    usize::from(end)
}

fn remove_item(out: &mut String, item: &SyntaxNode) {
    let start = item
        .prev_sibling_or_token()
        .filter(|t| t.kind() == SyntaxKind::TOKEN_WHITESPACE)
        .map(|t| t.text_range().start())
        .unwrap_or(item.text_range().start());
    out.replace_range(usize::from(start)..item_end(item), "");
}

fn insert_item(out: &mut String, list: &ast::List, text: &str, name: &str) {
    let items: Vec<SyntaxNode> = list.items().map(|i| i.syntax().clone()).collect();
    let range = list.syntax().text_range();
    let Some(first) = items.first() else {
        out.replace_range(
            usize::from(range.start()) + 1..usize::from(range.end()) - 1,
            &format!(" {text} "),
        );
        return;
    };
    let separator = first
        .prev_sibling_or_token()
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
            usize::from(first.text_range().start()),
            &format!("{text}{separator}"),
        ),
    }
}

fn edit_once(src: &str, spec: &ListSpec, name: &str, add: bool) -> Result<String> {
    let list = locate(src, spec)?
        .ok_or_else(|| anyhow!("{} disappeared while editing", spec.describe()))?;
    let mut out = src.to_string();
    if !add {
        if let Some(item) = list
            .items()
            .map(|i| i.syntax().clone())
            .find(|i| item_name(i) == name)
        {
            remove_item(&mut out, &item);
        }
        return Ok(out);
    }
    let in_with = list
        .syntax()
        .ancestors()
        .take_while(|n| n.kind() != SyntaxKind::NODE_ATTRPATH_VALUE)
        .any(|n| n.kind() == SyntaxKind::NODE_WITH);
    let prefixed = !in_with
        || list
            .items()
            .any(|i| i.syntax().text().to_string().starts_with("pkgs."));
    let text = match (spec.quoted, prefixed) {
        (true, _) => format!("\"{name}\""),
        (false, true) => format!("pkgs.{name}"),
        (false, false) => name.to_string(),
    };
    insert_item(&mut out, &list, &text, name);
    Ok(out)
}

pub fn edit_list(
    src: &str,
    spec: &ListSpec,
    names: &[String],
    add: bool,
) -> Result<Option<(String, Vec<String>)>> {
    let Some(list) = locate(src, spec)? else {
        return Ok(None);
    };
    let mut present: HashSet<String> = list.items().map(|i| item_name(i.syntax())).collect();
    let mut out = src.to_string();
    let mut changed = Vec::new();
    for name in names {
        if present.contains(name) == add {
            continue;
        }
        out = edit_once(&out, spec, name, add)?;
        if add {
            present.insert(name.clone());
        } else {
            present.remove(name);
        }
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
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(ft) = entry.file_type() else { continue };
            if name.starts_with('.') || ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                stack.push(entry.path());
            } else if name.ends_with(".nix") {
                files.push(entry.path());
            }
        }
    }
    files.sort();
    files
}

pub fn change(e: &Env, names: &[String], app: bool, add: bool) -> Result<()> {
    if names.is_empty() {
        bail!(
            "usage: macup {} [--app] <name>...",
            if add { "add" } else { "remove" }
        );
    }
    ensure_config(e)?;
    let spec = if app { &CASKS } else { &PACKAGES };
    let mut hits = Vec::new();
    let needle = spec.path.last().expect("list specs have a path");
    for file in nix_files(&e.nix_dir()) {
        let src = std::fs::read_to_string(&file)?;
        if !src.contains(needle) {
            continue;
        }
        if let Some(edit) =
            edit_list(&src, spec, names, add).with_context(|| format!("in {}", file.display()))?
        {
            hits.push((file, edit));
        }
    }
    let (file, (out, changed)) = match hits.len() {
        0 => bail!(
            "could not find a {} list in {}; edit it by hand with `macup edit`",
            spec.describe(),
            e.nix_dir().display()
        ),
        1 => hits.remove(0),
        _ => bail!(
            "{} is defined in several files ({}); edit it by hand with `macup edit`",
            spec.describe(),
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
    } else {
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
    }
    if app && !add {
        return offer_uninstall(names);
    }
    if changed.is_empty() {
        return Ok(());
    }
    offer_apply(e)
}

fn offer_uninstall(names: &[String]) -> Result<()> {
    let Some(brew) = crate::rsync::which("brew") else {
        return Ok(());
    };
    for name in names {
        let installed = Command::new(&brew)
            .args(["list", "--cask", name])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !installed {
            continue;
        }
        let keep = format!("{name} is still installed; `brew uninstall --cask {name}` removes it");
        if !interactive() || !confirm(&format!("Also uninstall {name} from this Mac?"), true)? {
            note(keep);
            continue;
        }
        if !Command::new(&brew)
            .args(["uninstall", "--cask", name])
            .status()?
            .success()
        {
            bail!("Homebrew could not uninstall {name}");
        }
        ok(format!("Uninstalled {name}"));
    }
    Ok(())
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
    let eval = |attr: &str| {
        Command::new(&nix)
            .args(nix_args(&["eval", "--raw", "--inputs-from", &dir, attr]))
            .output()
    };
    let out = eval(&format!("nixpkgs#{name}.name"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if !stderr.contains("does not provide attribute") {
            bail!(
                "could not look up {name:?} in nixpkgs:\n{}",
                stderr
                    .lines()
                    .filter(|l| !l.starts_with("fetching "))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
        let release = eval("nixpkgs#lib.trivial.release")
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
        bail!("{}", missing_package(name, release.as_deref()));
    }
    note(format!(
        "found {}",
        String::from_utf8_lossy(&out.stdout).trim()
    ));
    Ok(())
}

fn missing_package(name: &str, release: Option<&str>) -> String {
    let url = search::web_url(name);
    let hits = with_spinner("Searching search.nixos.org", || {
        Ok(search::packages(release, name))
    })
    .ok()
    .flatten()
    .filter(|hits| !hits.is_empty());
    match hits {
        Some(hits) => format!(
            "nixpkgs has no package called {name:?}. Closest matches on search.nixos.org:\n{}\nMore at {url}",
            search::format(&hits)
        ),
        None => format!("nixpkgs has no package called {name:?} (search at {url})"),
    }
}

#[cfg(test)]
#[path = "packages_tests.rs"]
mod tests;
