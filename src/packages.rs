use crate::env::Env;
use crate::nix::{ensure_config, find_nix, nix_args, offer_apply};
use crate::search::{self, Lookup};
use crate::ui::{confirm, interactive, note, ok, warn, with_spinner};
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
pub const BREWS: ListSpec = ListSpec {
    path: &["homebrew", "brews"],
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Tool,
    Formula,
    App,
}

const PREFERENCE: [Kind; 3] = [Kind::App, Kind::Formula, Kind::Tool];

impl Kind {
    fn spec(self) -> &'static ListSpec {
        match self {
            Kind::Tool => &PACKAGES,
            Kind::Formula => &BREWS,
            Kind::App => &CASKS,
        }
    }

    fn what(self) -> &'static str {
        match self {
            Kind::Tool | Kind::Formula => "command-line tool",
            Kind::App => "Mac app",
        }
    }

    fn source(self) -> &'static str {
        match self {
            Kind::Tool => "nixpkgs",
            Kind::Formula | Kind::App => "Homebrew",
        }
    }

    fn flag(self) -> &'static str {
        match self {
            Kind::Tool => "--tool",
            Kind::Formula => "--brew",
            Kind::App => "--app",
        }
    }
}

struct Workspace {
    dir: PathBuf,
    files: Vec<(PathBuf, String)>,
    dirty: HashSet<usize>,
}

impl Workspace {
    fn open(dir: &Path) -> Result<Workspace> {
        let files = nix_files(dir)
            .into_iter()
            .map(|f| {
                let src = std::fs::read_to_string(&f)?;
                Ok((f, src))
            })
            .collect::<Result<_>>()?;
        Ok(Workspace {
            dir: dir.to_path_buf(),
            files,
            dirty: HashSet::new(),
        })
    }

    fn find(&self, spec: &ListSpec) -> Result<Option<usize>> {
        let needle = spec.path.last().expect("list specs have a path");
        let mut hits = Vec::new();
        for (i, (file, src)) in self.files.iter().enumerate() {
            if src.contains(needle)
                && locate(src, spec)
                    .with_context(|| format!("in {}", file.display()))?
                    .is_some()
            {
                hits.push(i);
            }
        }
        match hits.as_slice() {
            [] => Ok(None),
            [only] => Ok(Some(*only)),
            _ => bail!(
                "{} is defined in several files ({}); edit it by hand with `macup edit`",
                spec.describe(),
                hits.iter()
                    .map(|&i| self.shown(i))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    fn contains(&self, spec: &ListSpec, name: &str) -> Result<bool> {
        let Some(i) = self.find(spec)? else {
            return Ok(false);
        };
        Ok(locate(&self.files[i].1, spec)?
            .is_some_and(|list| list.items().any(|item| item_name(item.syntax()) == name)))
    }

    fn edit(
        &mut self,
        spec: &ListSpec,
        names: &[String],
        add: bool,
    ) -> Result<Option<(String, Vec<String>)>> {
        let Some(i) = self.find(spec)? else {
            return Ok(None);
        };
        let (out, changed) = edit_list(&self.files[i].1, spec, names, add)?
            .expect("find only returns files with the list");
        if !changed.is_empty() {
            self.files[i].1 = out;
            self.dirty.insert(i);
        }
        Ok(Some((self.shown(i), changed)))
    }

    fn save(&self) -> Result<()> {
        for &i in &self.dirty {
            let (file, src) = &self.files[i];
            std::fs::write(file, src)?;
        }
        Ok(())
    }

    fn shown(&self, i: usize) -> String {
        let file = &self.files[i].0;
        file.strip_prefix(&self.dir)
            .unwrap_or(file)
            .display()
            .to_string()
    }
}

pub fn counts(dir: &Path) -> Option<(usize, usize)> {
    let ws = Workspace::open(dir).ok()?;
    let count = |spec: &ListSpec| {
        ws.find(spec)
            .ok()
            .flatten()
            .and_then(|i| locate(&ws.files[i].1, spec).ok().flatten())
            .map_or(0, |list| list.items().count())
    };
    Some((count(&PACKAGES) + count(&BREWS), count(&CASKS)))
}

pub fn add(e: &Env, names: &[String], forced: Option<Kind>) -> Result<()> {
    ensure_config(e)?;
    let mut ws = Workspace::open(&e.nix_dir())?;
    let mut picked: Vec<(Kind, String)> = Vec::new();
    for name in names {
        if [&PACKAGES, &CASKS, &BREWS]
            .into_iter()
            .map(|spec| ws.contains(spec, name))
            .collect::<Result<Vec<_>>>()?
            .contains(&true)
        {
            ok(format!("{name} is already in your config"));
            continue;
        }
        picked.push((kind_of(e, name, forced)?, name.clone()));
    }
    let mut added = Vec::new();
    for kind in [Kind::Tool, Kind::Formula, Kind::App] {
        let list: Vec<String> = picked
            .iter()
            .filter(|(k, _)| *k == kind)
            .map(|(_, name)| name.clone())
            .collect();
        if list.is_empty() {
            continue;
        }
        let spec = kind.spec();
        let (file, changed) = ws.edit(spec, &list, true)?.ok_or_else(|| {
            anyhow!(
                "could not find a {} list in {}; edit it by hand with `macup edit`",
                spec.describe(),
                e.nix_dir().display()
            )
        })?;
        if !changed.is_empty() {
            added.push(format!("Added {} in {file}", changed.join(", ")));
        }
    }
    ws.save()?;
    if added.is_empty() {
        return Ok(());
    }
    added.iter().for_each(ok);
    offer_apply(e)
}

pub fn remove(e: &Env, names: &[String], forced: Option<Kind>) -> Result<()> {
    ensure_config(e)?;
    let mut ws = Workspace::open(&e.nix_dir())?;
    let specs: Vec<&ListSpec> = match forced {
        Some(kind) => vec![kind.spec()],
        None => vec![&PACKAGES, &CASKS, &BREWS],
    };
    let mut removed = Vec::new();
    let mut found: HashSet<String> = HashSet::new();
    let mut tools_changed = false;
    let mut uninstall: Vec<(String, Brew)> = Vec::new();
    for spec in specs {
        let Some((file, changed)) = ws.edit(spec, names, false)? else {
            continue;
        };
        if changed.is_empty() {
            continue;
        }
        removed.push(format!("Removed {} from {file}", changed.join(", ")));
        match brew_kind(spec) {
            Some(kind) => uninstall.extend(changed.iter().map(|n| (n.clone(), kind))),
            None => tools_changed = true,
        }
        found.extend(changed);
    }
    ws.save()?;
    removed.iter().for_each(ok);
    let brews: Vec<Brew> = match forced {
        Some(kind) => brew_kind(kind.spec()).into_iter().collect(),
        None => vec![Brew::Cask, Brew::Formula],
    };
    for name in names.iter().filter(|n| !found.contains(*n)) {
        match installed_by_brew(name, &brews) {
            Some(kind) => {
                note(format!(
                    "{name} is not in your config, but Homebrew installed it"
                ));
                uninstall.push((name.clone(), kind));
            }
            None => warn(format!("{name} is not in your config")),
        }
    }
    offer_uninstall(&uninstall)?;
    if tools_changed {
        return offer_apply(e);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Brew {
    Cask,
    Formula,
}

impl Brew {
    fn flag(self) -> &'static str {
        match self {
            Brew::Cask => "--cask",
            Brew::Formula => "--formula",
        }
    }

    fn api(self) -> &'static str {
        match self {
            Brew::Cask => "cask",
            Brew::Formula => "formula",
        }
    }
}

fn brew_kind(spec: &ListSpec) -> Option<Brew> {
    if spec.path == CASKS.path {
        Some(Brew::Cask)
    } else if spec.path == BREWS.path {
        Some(Brew::Formula)
    } else {
        None
    }
}

fn installed_by_brew(name: &str, kinds: &[Brew]) -> Option<Brew> {
    if kinds.is_empty() {
        return None;
    }
    let brew = crate::rsync::which("brew")?;
    kinds.iter().copied().find(|kind| {
        Command::new(&brew)
            .args(["list", kind.flag(), name])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

fn offer_uninstall(items: &[(String, Brew)]) -> Result<()> {
    let Some(brew) = crate::rsync::which("brew") else {
        return Ok(());
    };
    for (name, kind) in items {
        if installed_by_brew(name, &[*kind]).is_none() {
            continue;
        }
        let flag = kind.flag();
        if !interactive() || !confirm(&format!("Also uninstall {name} from this Mac?"), true)? {
            note(format!(
                "{name} is still installed; `brew uninstall {flag} {name}` removes it"
            ));
            continue;
        }
        if !Command::new(&brew)
            .args(["uninstall", flag, name])
            .status()?
            .success()
        {
            bail!("Homebrew could not uninstall {name}");
        }
        ok(format!("Uninstalled {name}"));
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum Decision {
    Use(Kind),
    NotFound,
    CannotTell,
}

fn decide(lookups: &[(Kind, Lookup)]) -> Decision {
    if let Some((kind, _)) = lookups
        .iter()
        .find(|(_, lookup)| matches!(lookup, Lookup::Found(_)))
    {
        return Decision::Use(*kind);
    }
    if lookups.iter().all(|(_, lookup)| *lookup == Lookup::Missing) {
        Decision::NotFound
    } else {
        Decision::CannotTell
    }
}

fn describe(name: &str, kind: Kind, lookup: &Lookup) {
    let what = format!("{} from {}", kind.what(), kind.source());
    match lookup {
        Lookup::Found(detail) if !detail.is_empty() => note(format!("{name}: {what} ({detail})")),
        _ => note(format!("{name}: {what}")),
    }
}

fn lookup(e: &Env, name: &str, kind: Kind) -> Lookup {
    match kind {
        Kind::Tool => lookup_tool(e, name),
        Kind::Formula => lookup_brew(Brew::Formula, name),
        Kind::App => lookup_brew(Brew::Cask, name),
    }
}

fn kind_of(e: &Env, name: &str, forced: Option<Kind>) -> Result<Kind> {
    if let Some(kind) = forced {
        let found = with_spinner(&format!("Looking up {name}…"), || {
            Ok(lookup(e, name, kind))
        })?;
        match &found {
            Lookup::Missing => bail!("{}", not_found(e, name, Some(kind))),
            Lookup::Unknown(why) => warn(format!("could not check {name}: {why}")),
            Lookup::Found(_) => describe(name, kind, &found),
        }
        return Ok(kind);
    }
    let lookups: Vec<(Kind, Lookup)> = with_spinner(&format!("Looking up {name}…"), || {
        Ok(std::thread::scope(|s| {
            PREFERENCE
                .map(|kind| (kind, s.spawn(move || lookup(e, name, kind))))
                .into_iter()
                .map(|(kind, handle)| {
                    let found = handle.join().unwrap_or_else(|_| {
                        Lookup::Unknown(format!("the {} lookup crashed", kind.source()))
                    });
                    (kind, found)
                })
                .collect()
        }))
    })?;
    match decide(&lookups) {
        Decision::Use(kind) => {
            let mut preferred = true;
            for (other, found) in &lookups {
                if *other == kind {
                    describe(name, kind, found);
                    preferred = false;
                    continue;
                }
                match found {
                    Lookup::Found(_) => note(format!(
                        "{} also has a {} called {name}; `macup add {} {name}` adds that instead",
                        other.source(),
                        other.what(),
                        other.flag()
                    )),
                    Lookup::Unknown(why) if preferred => note(format!(
                        "could not check {} for a {}: {why}",
                        other.source(),
                        other.what()
                    )),
                    _ => {}
                }
            }
            Ok(kind)
        }
        Decision::NotFound => bail!("{}", not_found(e, name, None)),
        Decision::CannotTell => {
            let why: Vec<String> = lookups
                .iter()
                .filter_map(|(kind, found)| match found {
                    Lookup::Unknown(why) => {
                        Some(format!("{}s in {}: {why}", kind.what(), kind.source()))
                    }
                    _ => None,
                })
                .collect();
            bail!(
                "could not tell whether {name} is a command-line tool or a Mac app ({}); say which with --app, --brew or --tool",
                why.join("; ")
            )
        }
    }
}

fn nix_eval(e: &Env, nix: &Path, attr: &str) -> std::result::Result<String, String> {
    let dir = e.nix_dir().display().to_string();
    let out = Command::new(nix)
        .args(nix_args(&["eval", "--raw", "--inputs-from", &dir, attr]))
        .output()
        .map_err(|err| err.to_string())?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).trim().to_string());
    }
    Err(String::from_utf8_lossy(&out.stderr)
        .lines()
        .filter(|l| !l.starts_with("fetching "))
        .collect::<Vec<_>>()
        .join("\n"))
}

fn lookup_tool(e: &Env, name: &str) -> Lookup {
    let Some(nix) = find_nix() else {
        return Lookup::Unknown("Nix is not installed yet".into());
    };
    match nix_eval(e, &nix, &format!("nixpkgs#{name}.name")) {
        Ok(found) => Lookup::Found(found),
        Err(err) if err.contains("does not provide attribute") => Lookup::Missing,
        Err(err) => Lookup::Unknown(err),
    }
}

fn lookup_brew(kind: Brew, name: &str) -> Lookup {
    let Some(brew) = crate::rsync::which("brew") else {
        return search::brew_from_api(kind.api(), name);
    };
    let out = match Command::new(brew)
        .args(["info", kind.flag(), "--json=v2", name])
        .output()
    {
        Ok(out) => out,
        Err(err) => return Lookup::Unknown(err.to_string()),
    };
    if out.status.success() {
        return Lookup::Found(search::brew_version(&out.stdout).unwrap_or_default());
    }
    let err = String::from_utf8_lossy(&out.stderr).to_lowercase();
    if err.contains("no cask with this name")
        || err.contains(&format!("no available {}", kind.api()))
    {
        Lookup::Missing
    } else {
        Lookup::Unknown(err.trim().to_string())
    }
}

fn not_found(e: &Env, name: &str, kind: Option<Kind>) -> String {
    let url = search::web_url(name);
    let wanted = |k: Kind| kind.is_none_or(|kind| kind == k);
    let (tools, brews, apps) = with_spinner("Looking for similar names…", || {
        let tools = wanted(Kind::Tool)
            .then(|| {
                let release = find_nix()
                    .and_then(|nix| nix_eval(e, &nix, "nixpkgs#lib.trivial.release").ok());
                search::packages(release.as_deref(), name)
            })
            .flatten()
            .unwrap_or_default();
        let similar = |k: Kind, brew: Brew| {
            if wanted(k) {
                search::similar_brews(&e.home, brew.api(), name)
            } else {
                Vec::new()
            }
        };
        Ok((
            tools,
            similar(Kind::Formula, Brew::Formula),
            similar(Kind::App, Brew::Cask),
        ))
    })
    .unwrap_or_default();
    let mut msg = match kind {
        Some(kind) => format!("{} has no {} called {name:?}.", kind.source(), kind.what()),
        None => format!("neither nixpkgs nor Homebrew has anything called {name:?}."),
    };
    if !tools.is_empty() {
        msg.push_str(&format!(
            "\nCommand-line tools on search.nixos.org:\n{}",
            search::format(&tools)
        ));
    }
    if !brews.is_empty() {
        msg.push_str(&format!(
            "\nCommand-line tools on Homebrew:\n  {}",
            brews.join("\n  ")
        ));
    }
    if !apps.is_empty() {
        msg.push_str(&format!("\nMac apps on Homebrew:\n  {}", apps.join("\n  ")));
    }
    match kind {
        Some(Kind::App | Kind::Formula) if apps.is_empty() && brews.is_empty() => {
            msg.push_str(&format!(" Try `brew search {name}`."))
        }
        Some(Kind::App | Kind::Formula) => {}
        _ => msg.push_str(&format!("\nMore at {url}")),
    }
    msg
}

#[cfg(test)]
#[path = "packages_tests.rs"]
mod tests;
