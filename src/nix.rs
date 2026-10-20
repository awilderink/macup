use crate::config::{Config, InstallConfig};
use crate::env::{Env, Nix, has_flake};
use crate::picker::shell_preset;
use crate::selection::existing_paths;
use crate::ui::{confirm, interactive, note, ok, step, warn};
use crate::volume::check_mounted;
use anyhow::{Context, Result, anyhow, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

const FLAKE: &str = include_str!("flake.nix");
const HOST: &str = "macup";

pub fn require_nix(e: &Env) -> Result<()> {
    match e.nix {
        Nix::On => Ok(()),
        Nix::OffByConfig => bail!(
            "Nix is turned off for this drive ([nix] enabled = false in macup.toml); macup only backs up and restores here"
        ),
        Nix::Absent => bail!(
            "there is no Nix here: Nix is not installed and {} lists no tools or apps",
            e.drive_config().display()
        ),
    }
}

pub fn find_nix() -> Option<PathBuf> {
    crate::rsync::which("nix").or_else(|| {
        [
            "/nix/var/nix/profiles/default/bin/nix",
            "/run/current-system/sw/bin/nix",
        ]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.exists())
    })
}

pub fn nix_args(args: &[&str]) -> Vec<String> {
    let mut all = vec![
        "--extra-experimental-features".to_string(),
        "nix-command flakes".to_string(),
    ];
    all.extend(args.iter().map(|a| a.to_string()));
    all
}

pub fn nix_version(nix: &Path) -> String {
    Command::new(nix)
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .next()
                .map(str::to_string)
        })
        .unwrap_or_else(|| "unknown".into())
}

fn run_attached(program: &str, args: &[String]) -> Result<()> {
    note(format!("$ {program} {}", args.join(" ")));
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("starting {program}"))?;
    if !status.success() {
        bail!("{program} exited with {status}");
    }
    Ok(())
}

fn path_str(p: &Path) -> String {
    p.display().to_string()
}

pub fn drive_config(e: &Env) -> Result<Config> {
    check_mounted(&e.volume)?;
    if !e.drive_config().is_file() {
        bail!(
            "{} has no {}, which lists your tools and apps",
            e.volume.display(),
            crate::config::FILE_NAME
        );
    }
    Config::load(&e.volume)
}

pub fn prepare(e: &Env) -> Result<PathBuf> {
    if let Some(dir) = e.own_flake() {
        if !has_flake(&dir) {
            bail!(
                "macup.toml's [nix] config points at {}, but there is no flake.nix there",
                dir.display()
            );
        }
        return Ok(dir);
    }
    let install = drive_config(e)?.install;
    let lock = std::fs::read(e.drive_lock()).ok();
    write_flake(&e.flake_dir(), &e.user(), &install, lock.as_deref())?;
    Ok(e.flake_dir())
}

pub fn write_flake(
    dir: &Path,
    user: &str,
    install: &InstallConfig,
    lock: Option<&[u8]>,
) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let mac = serde_json::json!({
        "user": user,
        "brew": install.brew,
        "cask": install.cask,
        "nix": install.nix,
    });
    replace(&dir.join("flake.nix"), FLAKE.as_bytes())?;
    replace(
        &dir.join("mac.json"),
        serde_json::to_string_pretty(&mac)?.as_bytes(),
    )?;
    if let Some(lock) = lock {
        replace(&dir.join("flake.lock"), lock)?;
    }
    Ok(())
}

fn replace(path: &Path, contents: &[u8]) -> Result<()> {
    if std::fs::read(path).is_ok_and(|old| old == contents) {
        return Ok(());
    }
    let _ = std::fs::remove_file(path);
    std::fs::write(path, contents).with_context(|| format!("writing {}", path.display()))
}

fn save_lock(e: &Env, dir: &Path) {
    if e.own_flake().is_some() {
        return;
    }
    let Ok(lock) = std::fs::read(dir.join("flake.lock")) else {
        return;
    };
    if std::fs::read(e.drive_lock()).is_ok_and(|old| old == lock) {
        return;
    }
    match std::fs::write(e.drive_lock(), lock) {
        Ok(()) => ok(format!(
            "pinned versions saved to {}",
            e.drive_lock().display()
        )),
        Err(err) => warn(format!(
            "could not save the pinned versions to {}: {err}",
            e.drive_lock().display()
        )),
    }
}

fn lock(nix: &str, dir: &str) -> Result<()> {
    let out = Command::new(nix)
        .args(nix_args(&["flake", "lock", dir]))
        .output()
        .with_context(|| format!("starting {nix}"))?;
    if !out.status.success() {
        bail!(
            "pinning versions failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

fn flake_host(nix: &Path, dir: &Path) -> Result<String> {
    let target = format!("{}#darwinConfigurations", path_str(dir));
    let out = Command::new(nix)
        .args(nix_args(&[
            "eval",
            "--json",
            &target,
            "--apply",
            "builtins.attrNames",
        ]))
        .output()?;
    if !out.status.success() {
        bail!(
            "cannot read darwinConfigurations from {}: {}",
            dir.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let names: Vec<String> = serde_json::from_slice(&out.stdout)?;
    if let [only] = names.as_slice() {
        return Ok(only.clone());
    }
    let local = crate::env::this_mac_name();
    if names.contains(&local) {
        return Ok(local);
    }
    bail!(
        "flake.nix defines {} and none matches this Mac's name; keep one or rename this Mac",
        names.join(", ")
    )
}

pub fn apply(e: &Env, update: bool, check: bool) -> Result<()> {
    install_and_apply(e, update, check, "macup apply")?;
    if check {
        return Ok(());
    }
    offer_shell_restore(e)
}

pub fn install_and_apply(e: &Env, update: bool, check: bool, rerun: &str) -> Result<()> {
    if std::env::consts::ARCH != "aarch64" {
        bail!(
            "macup's setup targets Apple Silicon; on Intel, point [nix] config in macup.toml at your own flake"
        );
    }
    step("Apple Command Line Tools");
    if !Command::new("xcode-select")
        .arg("-p")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        let _ = run_attached("xcode-select", &["--install".into()]);
        bail!("finish Apple's Command Line Tools installer, then run `{rerun}` again");
    }
    ok("installed");

    step("Nix");
    let nix = match find_nix() {
        Some(nix) => nix,
        None => {
            note("installing Lix, a Nix implementation");
            run_attached("/bin/sh", &["-c".into(), "curl --proto '=https' --tlsv1.2 -sSf -L https://install.lix.systems/lix | sh -s -- install".into()])
                .context("installing Lix failed")?;
            find_nix().ok_or_else(|| anyhow!("Lix was installed but is not on PATH yet — open a new terminal and run `{rerun}` again"))?
        }
    };
    ok(nix_version(&nix));

    step("Configuration");
    let dir = prepare(e)?;
    let host = match e.own_flake() {
        Some(_) => flake_host(&nix, &dir)?,
        None => HOST.to_string(),
    };
    match e.own_flake() {
        Some(_) => ok(format!("{}#{host}", dir.display())),
        None => ok(format!(
            "tools and apps from {}",
            e.drive_config().display()
        )),
    }
    let nix_s = path_str(&nix);
    let dir_s = path_str(&dir);

    if update {
        step("Updating pinned packages");
        run_attached(&nix_s, &nix_args(&["flake", "update", "--flake", &dir_s]))
            .context("updating failed")?;
    } else {
        lock(&nix_s, &dir_s)?;
    }
    save_lock(e, &dir);

    if check {
        step("Building (check only, nothing is changed)");
        let target = format!("{dir_s}#darwinConfigurations.{host}.system");
        run_attached(&nix_s, &nix_args(&["build", "--no-link", &target]))
            .context("the configuration does not build")?;
        ok("configuration builds");
        return Ok(());
    }

    step("Applying system configuration (sudo may ask for your password)");
    let flake = format!("{dir_s}#{host}");
    let mut args = vec!["-H".to_string(), nix_s.clone()];
    args.extend(nix_args(&[
        "run",
        "--inputs-from",
        &dir_s,
        "nix-darwin#darwin-rebuild",
        "--",
        "switch",
        "--flake",
        &flake,
    ]));
    run_attached("sudo", &args).context("applying failed")?;
    ok("system configuration applied");
    Ok(())
}

fn offer_shell_restore(e: &Env) -> Result<()> {
    if e.home.join(".oh-my-zsh").exists() {
        return Ok(());
    }
    let Ok(src) = crate::machines::open(e, None, false) else {
        return Ok(());
    };
    let paths = existing_paths(&src.backup_home(), &shell_preset().paths);
    if paths.is_empty() {
        return Ok(());
    }
    if !interactive() {
        note(format!(
            "restore your shell config with: macup restore --path {}",
            paths.join(" --path ")
        ));
        return Ok(());
    }
    if !confirm(
        &format!(
            "Restore your shell config from the backup? ({})",
            paths.join(", ")
        ),
        true,
    )? {
        return Ok(());
    }
    crate::restore::shell_restore(&src, &paths)
}

pub fn edit(e: &Env) -> Result<()> {
    drive_config(e)?;
    crate::packages::upgrade_file(e)?;
    let before = drive_config(e)?.install;
    let mut editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_default();
    if editor.is_empty() {
        editor = if crate::rsync::which("code").is_some() {
            "code --wait".into()
        } else {
            "vi".into()
        };
    }
    loop {
        run_attached(
            "/bin/sh",
            &[
                "-c".into(),
                format!("{editor} \"$1\""),
                "macup-edit".into(),
                path_str(&e.drive_config()),
            ],
        )?;
        let checked = drive_config(e).and_then(|after| {
            if e.own_flake().is_some() {
                return Ok(());
            }
            crate::packages::check_edited(e, &before, &after.install)
        });
        match checked {
            Ok(()) => break,
            Err(err) if interactive() => {
                warn(format!("{err:#}"));
                if !confirm("Open macup.toml again to fix it?", true)? {
                    return Err(err);
                }
            }
            Err(err) => return Err(err),
        }
    }
    offer_apply(e)
}

pub fn offer_apply(e: &Env) -> Result<()> {
    if !interactive() {
        note("run `macup apply` to make the change take effect");
        return Ok(());
    }
    if !confirm("Apply the change now?", true)? {
        note("run `macup apply` later to make the change take effect");
        return Ok(());
    }
    apply(e, false, false)
}

#[cfg(test)]
#[path = "nix_tests.rs"]
mod tests;
