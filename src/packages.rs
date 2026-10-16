use crate::env::Env;
use crate::nix::{ensure_config, find_nix, nix_args, offer_apply};
use crate::ui::{note, ok};
use anyhow::{Result, bail};
use regex::Regex;
use std::process::Command;
use std::sync::LazyLock;

static PACKAGES_BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)(home\.packages = with pkgs; \[\n)(.*?)(\n([ \t]*)\];)").expect("valid regex")
});
static CASKS_LIST: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(casks = \[)([^\]]*)(\];)").expect("valid regex"));

pub type Edit = fn(&str, &[String], bool) -> Result<(String, Vec<String>)>;

pub fn edit_packages(src: &str, names: &[String], add: bool) -> Result<(String, Vec<String>)> {
    let Some(m) = PACKAGES_BLOCK.captures(src) else {
        bail!(
            "could not find `home.packages = with pkgs; [ … ];` in home.nix — edit it by hand with `macup edit`"
        );
    };
    let body = m.get(2).expect("group 2");
    let indent = format!("{}  ", &m[4]);
    let current: Vec<String> = body.as_str().split_whitespace().map(String::from).collect();
    let (next, changed) = apply_change(current, names, add);
    if changed.is_empty() {
        return Ok((src.to_string(), changed));
    }
    let lines: Vec<String> = next.iter().map(|n| format!("{indent}{n}")).collect();
    Ok((
        format!(
            "{}{}{}",
            &src[..body.start()],
            lines.join("\n"),
            &src[body.end()..]
        ),
        changed,
    ))
}

pub fn edit_casks(src: &str, names: &[String], add: bool) -> Result<(String, Vec<String>)> {
    let Some(m) = CASKS_LIST.captures(src) else {
        bail!(
            "could not find `casks = [ … ];` in flake.nix — edit it by hand with `macup edit apps`"
        );
    };
    let body = m.get(2).expect("group 2");
    let current: Vec<String> = body
        .as_str()
        .split_whitespace()
        .filter_map(|f| {
            f.strip_prefix('"')
                .and_then(|f| f.strip_suffix('"'))
                .map(String::from)
        })
        .collect();
    let (next, changed) = apply_change(current, names, add);
    if changed.is_empty() {
        return Ok((src.to_string(), changed));
    }
    let quoted: Vec<String> = next.iter().map(|n| format!("\"{n}\"")).collect();
    Ok((
        format!(
            "{} {} {}",
            &src[..body.start()],
            quoted.join(" "),
            &src[body.end()..]
        ),
        changed,
    ))
}

fn apply_change(mut next: Vec<String>, names: &[String], add: bool) -> (Vec<String>, Vec<String>) {
    let mut changed = Vec::new();
    for n in names {
        let has = next.contains(n);
        if add && !has {
            next.push(n.clone());
            changed.push(n.clone());
        } else if !add && has {
            next.retain(|s| s != n);
            changed.push(n.clone());
        }
    }
    next.sort_by_key(|s| s.to_lowercase());
    (next, changed)
}

pub fn change(e: &Env, names: &[String], app: bool, add: bool) -> Result<()> {
    if names.is_empty() {
        bail!(
            "usage: macup {} [--app] <name>...",
            if add { "add" } else { "remove" }
        );
    }
    ensure_config(e)?;
    let (file, edit): (_, Edit) = if app {
        ("flake.nix", edit_casks)
    } else {
        ("home.nix", edit_packages)
    };
    let path = e.nix_dir().join(file);
    if add {
        for n in names {
            validate(e, n, app)?;
        }
    }
    let (out, changed) = edit(&std::fs::read_to_string(&path)?, names, add)?;
    if changed.is_empty() {
        ok("nothing to change");
        return Ok(());
    }
    std::fs::write(&path, out)?;
    ok(format!(
        "{} {} in {file}",
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
