use crate::access::{full_disk_access, terminal_name};
use crate::env::{Env, Nix};
use crate::nix::find_nix;
use crate::ui::{human_bytes, title, warning};
use crate::volume::{check_mounted, free_space, nearly_full};
use std::path::Path;

pub fn show(e: &Env) {
    let row = |label: &str, value: String| println!("{} {value}", title(format!("{label:<14}")));
    println!("{}", title("macup status"));
    let mounted = check_mounted(&e.volume).is_ok();
    if mounted {
        let mut v = e.volume.display().to_string();
        if let Ok((free, total)) = free_space(&e.volume) {
            v.push_str(&format!(
                " · {} free of {}",
                human_bytes(free),
                human_bytes(total)
            ));
            if nearly_full(free, total) {
                v = warning(format!("{v} · nearly full")).to_string();
            }
        }
        row("Backup drive", v);
        let others: Vec<String> = crate::machines::backed_up_macs(e)
            .into_iter()
            .filter(|m| *m != e.machine)
            .collect();
        let mut v = e.machine.clone();
        if !others.is_empty() {
            v.push_str(&format!(" · also on the drive: {}", others.join(", ")));
        }
        if crate::machines::has_legacy_layout(e) {
            v.push_str(
                &warning(
                    " · drive uses the old single-Mac layout; the next backup or restore moves it",
                )
                .to_string(),
            );
        }
        row("This Mac", v);
    } else {
        row("Backup drive", warning("not connected").to_string());
    }
    row("Last backup", crate::backup::last_backup(e));
    row("Photos", crate::photos::last_photo_backup(e));
    row(
        "Disk access",
        match full_disk_access(&e.home) {
            None => "unknown".into(),
            Some(true) => format!("{} has Full Disk Access", terminal_name()),
            Some(false) => warning(format!(
                "{} lacks Full Disk Access — backups will miss protected folders",
                terminal_name()
            ))
            .to_string(),
        },
    );
    row(
        "Tools & apps",
        match e.nix {
            Nix::On => tools_and_apps(e, mounted),
            Nix::OffByConfig => "not managed by macup (Nix is off in macup.toml)".into(),
            Nix::Absent => {
                "not managed by macup (Nix is not installed and macup.toml lists nothing)".into()
            }
        },
    );
}

fn tools_and_apps(e: &Env, mounted: bool) -> String {
    let install = &e.config.install;
    let mut v = match e.own_flake() {
        Some(dir) => format!("from your own flake at {}", dir.display()),
        None if !mounted => {
            return warning("connect the backup drive, which lists them").to_string();
        }
        None if install.is_empty() => "none listed yet — `macup add <name>` adds one".into(),
        None => install
            .lists()
            .iter()
            .map(|(installer, list)| format!("{} {installer}", list.len()))
            .collect::<Vec<_>>()
            .join(" · "),
    };
    if find_nix().is_none() || !Path::new("/run/current-system").exists() {
        v.push_str(&warning(" · not installed yet — run macup setup").to_string());
    }
    v
}
