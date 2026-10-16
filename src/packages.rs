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
    App,
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

fn require_names(names: &[String], verb: &str) -> Result<()> {
    if names.is_empty() {
        bail!("usage: macup {verb} <name>...");
    }
    Ok(())
}

pub fn add(e: &Env, names: &[String], forced: Option<Kind>) -> Result<()> {
    require_names(names, "add")?;
    ensure_config(e)?;
    let mut ws = Workspace::open(&e.nix_dir())?;
    let (mut tools, mut apps) = (Vec::new(), Vec::new());
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
        match kind_of(e, name, forced)? {
            Kind::Tool => tools.push(name.clone()),
            Kind::App => apps.push(name.clone()),
        }
    }
    let mut added = Vec::new();
    for (spec, list) in [(&PACKAGES, tools), (&CASKS, apps)] {
        if list.is_empty() {
            continue;
        }
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
    require_names(names, "remove")?;
    ensure_config(e)?;
    let mut ws = Workspace::open(&e.nix_dir())?;
    let specs: &[&ListSpec] = match forced {
        Some(Kind::Tool) => &[&PACKAGES],
        Some(Kind::App) => &[&CASKS],
        None => &[&PACKAGES, &CASKS, &BREWS],
    };
    let mut removed = Vec::new();
    let mut found: HashSet<String> = HashSet::new();
    let mut tools_changed = false;
    let mut uninstall: Vec<(String, Brew)> = Vec::new();
    for &spec in specs {
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
    for name in names.iter().filter(|n| !found.contains(*n)) {
        match (forced != Some(Kind::Tool))
            .then(|| installed_by_brew(name))
            .flatten()
        {
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

fn installed_by_brew(name: &str) -> Option<Brew> {
    let brew = crate::rsync::which("brew")?;
    [Brew::Cask, Brew::Formula].into_iter().find(|kind| {
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
        if installed_by_brew(name) != Some(*kind) {
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

fn decide(tool: &Lookup, app: &Lookup) -> Decision {
    match (tool, app) {
        (_, Lookup::Found(_)) => Decision::Use(Kind::App),
        (Lookup::Found(_), _) => Decision::Use(Kind::Tool),
        (Lookup::Missing, Lookup::Missing) => Decision::NotFound,
        _ => Decision::CannotTell,
    }
}

fn describe(name: &str, kind: Kind, lookup: &Lookup) {
    let what = match kind {
        Kind::Tool => "command-line tool from nixpkgs",
        Kind::App => "Mac app from Homebrew",
    };
    match lookup {
        Lookup::Found(detail) if !detail.is_empty() => note(format!("{name}: {what} ({detail})")),
        _ => note(format!("{name}: {what}")),
    }
}

fn kind_of(e: &Env, name: &str, forced: Option<Kind>) -> Result<Kind> {
    if let Some(kind) = forced {
        let lookup = with_spinner(&format!("Looking up {name}…"), || {
            Ok(match kind {
                Kind::Tool => lookup_tool(e, name),
                Kind::App => lookup_app(name),
            })
        })?;
        match &lookup {
            Lookup::Missing => bail!("{}", not_found(e, name, Some(kind))),
            Lookup::Unknown(why) => warn(format!("could not check {name}: {why}")),
            Lookup::Found(_) => describe(name, kind, &lookup),
        }
        return Ok(kind);
    }
    let (tool, app) = with_spinner(&format!("Looking up {name}…"), || {
        Ok(std::thread::scope(|s| {
            let app = s.spawn(|| lookup_app(name));
            let tool = lookup_tool(e, name);
            let app = app
                .join()
                .unwrap_or_else(|_| Lookup::Unknown("the Homebrew lookup crashed".into()));
            (tool, app)
        }))
    })?;
    match decide(&tool, &app) {
        Decision::Use(Kind::App) => {
            describe(name, Kind::App, &app);
            if let Lookup::Found(_) = tool {
                note(format!(
                    "nixpkgs also has a package called {name}; `macup add --tool {name}` adds that instead"
                ));
            }
            Ok(Kind::App)
        }
        Decision::Use(Kind::Tool) => {
            describe(name, Kind::Tool, &tool);
            if let Lookup::Unknown(why) = &app {
                note(format!("could not check Homebrew for an app: {why}"));
            }
            Ok(Kind::Tool)
        }
        Decision::NotFound => bail!("{}", not_found(e, name, None)),
        Decision::CannotTell => {
            let why: Vec<String> = [("nixpkgs", &tool), ("Homebrew", &app)]
                .into_iter()
                .filter_map(|(source, l)| match l {
                    Lookup::Unknown(why) => Some(format!("{source}: {why}")),
                    _ => None,
                })
                .collect();
            bail!(
                "could not tell whether {name} is a command-line tool or a Mac app ({}); say which with --tool or --app",
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

fn lookup_app(name: &str) -> Lookup {
    let Some(brew) = crate::rsync::which("brew") else {
        return search::cask_from_api(name);
    };
    let out = match Command::new(brew)
        .args(["info", "--cask", "--json=v2", name])
        .output()
    {
        Ok(out) => out,
        Err(err) => return Lookup::Unknown(err.to_string()),
    };
    if out.status.success() {
        return Lookup::Found(search::cask_version(&out.stdout).unwrap_or_default());
    }
    let err = String::from_utf8_lossy(&out.stderr).to_lowercase();
    if err.contains("no cask with this name") || err.contains("no available cask") {
        Lookup::Missing
    } else {
        Lookup::Unknown(err.trim().to_string())
    }
}

fn not_found(e: &Env, name: &str, kind: Option<Kind>) -> String {
    let url = search::web_url(name);
    let (tools, apps) = with_spinner("Looking for similar names…", || {
        let tools = (kind != Some(Kind::App))
            .then(|| {
                let release = find_nix()
                    .and_then(|nix| nix_eval(e, &nix, "nixpkgs#lib.trivial.release").ok());
                search::packages(release.as_deref(), name)
            })
            .flatten()
            .unwrap_or_default();
        let apps = if kind == Some(Kind::Tool) {
            Vec::new()
        } else {
            search::similar_casks(&e.home, name)
        };
        Ok((tools, apps))
    })
    .unwrap_or_default();
    let mut msg = match kind {
        Some(Kind::Tool) => format!("nixpkgs has no package called {name:?}."),
        Some(Kind::App) => format!("Homebrew has no app called {name:?}."),
        None => format!("neither nixpkgs nor Homebrew has anything called {name:?}."),
    };
    if !tools.is_empty() {
        msg.push_str(&format!(
            "\nCommand-line tools on search.nixos.org:\n{}",
            search::format(&tools)
        ));
    }
    if !apps.is_empty() {
        msg.push_str(&format!("\nMac apps on Homebrew:\n  {}", apps.join("\n  ")));
    }
    match kind {
        Some(Kind::App) if apps.is_empty() => msg.push_str(&format!(" Try `brew search {name}`.")),
        Some(Kind::App) => {}
        _ => msg.push_str(&format!("\nMore at {url}")),
    }
    msg
}

#[cfg(test)]
#[path = "packages_tests.rs"]
mod tests;
