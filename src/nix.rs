use crate::env::{Env, Nix, has_flake};
use crate::picker::shell_preset;
use crate::selection::existing_paths;
use crate::ui::{confirm, interactive, note, ok, select, step, warn};
use crate::volume::check_mounted;
use anyhow::{Context, Result, anyhow, bail};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn require_nix(e: &Env) -> Result<()> {
    match e.nix {
        Nix::On => Ok(()),
        Nix::OffByConfig => bail!(
            "Nix is turned off for this drive ([nix] enabled = false in macup.toml); macup only backs up and restores here"
        ),
        Nix::Absent => bail!(
            "there is no Nix here: Nix is not installed and there is no nix-darwin config in {} or on the backup drive",
            e.nix_dir().display()
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

pub fn flake_host(e: &Env, nix: &Path) -> Result<String> {
    let target = format!("{}#darwinConfigurations", path_str(&e.nix_dir()));
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
            e.nix_dir().display(),
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
            "this configuration targets Apple Silicon; update flake.nix before using it on Intel"
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
    ensure_config(e)?;
    let host = flake_host(e, &nix)?;
    ok(format!("{}#{host}", e.nix_dir().display()));
    let nix_s = path_str(&nix);
    let dir = path_str(&e.nix_dir());

    if update {
        ensure_lock_owned(e)?;
        step("Updating pinned packages");
        run_attached(&nix_s, &nix_args(&["flake", "update", "--flake", &dir]))
            .context("updating failed")?;
    }

    if check {
        step("Building (check only, nothing is changed)");
        let target = format!("{dir}#darwinConfigurations.{host}.system");
        run_attached(&nix_s, &nix_args(&["build", "--no-link", &target]))
            .context("the configuration does not build")?;
        ok("configuration builds");
        return Ok(());
    }

    step("Applying system configuration (sudo may ask for your password)");
    let flake = format!("{dir}#{host}");
    let mut args = vec!["-H".to_string(), nix_s.clone()];
    args.extend(nix_args(&[
        "run",
        "--inputs-from",
        &dir,
        "nix-darwin#darwin-rebuild",
        "--",
        "switch",
        "--flake",
        &flake,
    ]));
    run_attached("sudo", &args).context("applying failed")?;
    ensure_lock_owned(e)?;
    ok("system configuration applied");

    if check_mounted(&e.volume).is_ok()
        && let Ok(bin) = crate::rsync::find_rsync(e)
    {
        match sync_config_to_drive(e, &bin) {
            Ok(()) => ok(format!("config saved to {}", e.drive_nix_dir().display())),
            Err(err) => warn(format!("could not copy the config to the drive: {err:#}")),
        }
    }
    Ok(())
}

pub fn ensure_config(e: &Env) -> Result<()> {
    if e.has_nix_config() {
        return Ok(());
    }
    if !has_flake(&e.drive_nix_dir()) {
        bail!(
            "no config at {} and none on the backup drive ({}) — connect the drive",
            e.nix_dir().display(),
            e.drive_nix_dir().display()
        );
    }
    note("copying the config from the backup drive");
    copy_dir(&e.drive_nix_dir(), &e.nix_dir())
}

fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        let target = dst.join(entry.file_name());
        if ft.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else if ft.is_file() {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn ensure_lock_owned(e: &Env) -> Result<()> {
    let lock = e.nix_dir().join("flake.lock");
    let Ok(meta) = std::fs::metadata(&lock) else {
        return Ok(());
    };
    if meta.uid() == unsafe { libc::getuid() } {
        return Ok(());
    }
    note(format!(
        "flake.lock is owned by root; giving it back to {}",
        e.user()
    ));
    run_attached(
        "sudo",
        &[
            "chown".into(),
            unsafe { libc::getuid() }.to_string(),
            path_str(&lock),
        ],
    )
}

pub fn sync_config_to_drive(e: &Env, rsync_bin: &str) -> Result<()> {
    if !e.nix_enabled() || !e.has_nix_config() {
        return Ok(());
    }
    std::fs::create_dir_all(e.drive_nix_dir())?;
    let out = Command::new(rsync_bin)
        .args([
            "-a",
            "--delete",
            &crate::rsync::dir_arg(&e.nix_dir()),
            &crate::rsync::dir_arg(&e.drive_nix_dir()),
        ])
        .output()?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

pub fn config_in_sync(e: &Env) -> bool {
    fn same(a: &Path, b: &Path) -> bool {
        let Ok(entries) = std::fs::read_dir(a) else {
            return false;
        };
        entries.flatten().all(|entry| {
            let (pa, pb) = (entry.path(), b.join(entry.file_name()));
            match entry.file_type() {
                Ok(ft) if ft.is_dir() => same(&pa, &pb),
                Ok(_) => {
                    matches!((std::fs::read(&pa), std::fs::read(&pb)), (Ok(x), Ok(y)) if x == y)
                }
                Err(_) => false,
            }
        })
    }
    same(&e.nix_dir(), &e.drive_nix_dir())
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

pub fn edit(e: &Env, section: Option<String>) -> Result<()> {
    let section = match section {
        Some(s) => s,
        None if interactive() => select(
            "What do you want to change?",
            &[
                (
                    "Command-line tools (home.nix)".into(),
                    "packages".to_string(),
                ),
                (
                    "Apps installed with Homebrew (flake.nix)".into(),
                    "apps".to_string(),
                ),
                (
                    "macOS and Nix settings (darwin.nix)".into(),
                    "system".to_string(),
                ),
            ],
        )?,
        None => "packages".into(),
    };
    let file = match section.as_str() {
        "packages" => "home.nix",
        "apps" => "flake.nix",
        "system" => "darwin.nix",
        other => bail!("unknown section {other:?} (packages, apps or system)"),
    };
    ensure_config(e)?;
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
    run_attached(
        "/bin/sh",
        &[
            "-c".into(),
            format!("{editor} \"$1\""),
            "macup-edit".into(),
            path_str(&e.nix_dir().join(file)),
        ],
    )?;
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
