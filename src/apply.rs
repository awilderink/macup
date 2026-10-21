use crate::config::{Config, InstallConfig};
use crate::env::{Env, env_var};
use crate::picker::shell_preset;
use crate::selection::existing_paths;
use crate::ui::{confirm, interactive, note, ok, step, warn};
use crate::volume::check_mounted;
use anyhow::{Context, Result, anyhow, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

const HOMEBREW_INSTALLER: &str =
    "https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh";
const NIX_HOMEBREW_MARKER: &str = "/opt/homebrew/Library/.homebrew-is-managed-by-nix";

fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")?
        .to_string_lossy()
        .split(':')
        .map(|dir| Path::new(dir).join(name))
        .find(|p| p.is_file())
}

pub fn find_brew() -> Option<PathBuf> {
    which("brew").or_else(|| {
        ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"]
            .iter()
            .map(PathBuf::from)
            .find(|p| p.exists())
    })
}

pub fn installed(install: &InstallConfig) -> bool {
    install.is_empty() || find_brew().is_some()
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

pub fn brewfile(install: &InstallConfig) -> String {
    let mut out = String::new();
    for (kind, list) in install.lists() {
        for name in list {
            out.push_str(&format!("{} \"{name}\"\n", kind.key()));
        }
    }
    out
}

fn write_brewfile(e: &Env, install: &InstallConfig) -> Result<PathBuf> {
    let dir = e.cache_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let file = dir.join("Brewfile");
    std::fs::write(&file, brewfile(install))
        .with_context(|| format!("writing {}", file.display()))?;
    Ok(file)
}

pub fn apply(e: &Env, update: bool, check: bool) -> Result<()> {
    install_and_apply(e, update, check, "macup apply")?;
    if check {
        return Ok(());
    }
    offer_shell_restore(e)
}

pub fn install_and_apply(e: &Env, update: bool, check: bool, rerun: &str) -> Result<()> {
    let install = drive_config(e)?.install;
    if install.is_empty() {
        note(format!(
            "{} lists no tools or apps, so there is nothing to install",
            e.drive_config().display()
        ));
        return Ok(());
    }
    step("Apple Command Line Tools");
    if !Command::new("xcode-select")
        .arg("-p")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        if check {
            note("not installed yet; `macup apply` starts Apple's installer");
            return Ok(());
        }
        let _ = run_attached("xcode-select", &["--install".into()]);
        bail!("finish Apple's Command Line Tools installer, then run `{rerun}` again");
    }
    ok("installed");

    step("Homebrew");
    let brew = match find_brew() {
        Some(brew) => brew,
        None if check => {
            note("not installed yet; `macup apply` installs it");
            return Ok(());
        }
        None => install_homebrew(rerun)?,
    };
    ok(path_str(&brew));
    step("Tools and apps");
    let file = path_str(&write_brewfile(e, &install)?);
    let brew = path_str(&brew);
    if check {
        let out = Command::new(&brew)
            .args(["bundle", "check", "--verbose", &format!("--file={file}")])
            .output()
            .with_context(|| format!("starting {brew}"))?;
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        text.lines()
            .filter_map(|l| l.trim().strip_prefix("→ "))
            .for_each(note);
        if out.status.success() {
            ok("everything listed is installed");
        } else {
            note("`macup apply` installs what is missing");
        }
        return Ok(());
    }
    if update {
        run_attached(&brew, &["update".into()]).context("updating Homebrew failed")?;
    }
    let upgrade = if update { "--upgrade" } else { "--no-upgrade" };
    run_attached(
        &brew,
        &[
            "bundle".into(),
            "install".into(),
            upgrade.into(),
            format!("--file={file}"),
        ],
    )
    .context("Homebrew could not install everything")?;
    ok("tools and apps installed");
    Ok(())
}

fn install_homebrew(rerun: &str) -> Result<PathBuf> {
    if Path::new(NIX_HOMEBREW_MARKER).exists() {
        bail!(
            "Homebrew in /opt/homebrew is still the one Nix set up; finish moving this Mac off Nix, then run `{rerun}` again"
        );
    }
    note("installing Homebrew with its official installer");
    run_attached(
        "/bin/bash",
        &[
            "-c".into(),
            format!("/bin/bash -c \"$(/usr/bin/curl -fsSL {HOMEBREW_INSTALLER})\""),
        ],
    )
    .context("installing Homebrew failed")?;
    find_brew().ok_or_else(|| {
        anyhow!("Homebrew was installed but cannot be found — open a new terminal and run `{rerun}` again")
    })
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
    let editor = env_var("VISUAL")
        .or_else(|| env_var("EDITOR"))
        .unwrap_or_else(|| {
            if which("code").is_some() {
                "code --wait".into()
            } else {
                "vi".into()
            }
        });
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
        let checked = drive_config(e)
            .and_then(|after| crate::packages::check_edited(&before, &after.install));
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
#[path = "apply_tests.rs"]
mod tests;
