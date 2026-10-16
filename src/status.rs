use crate::access::{full_disk_access, terminal_name};
use crate::env::Env;
use crate::nix::{config_in_sync, find_nix, nix_version};
use crate::ui::{human_bytes, title, warning};
use crate::volume::{check_mounted, free_space, nearly_full};

pub fn show(e: &Env) {
    let row = |label: &str, value: String| println!("{} {value}", title(format!("{label:<14}")));
    println!("{}", title("macup status"));
    let mounted = check_mounted(&e.volume).is_ok();
    if mounted {
        let mut v = format!("{} · connected", e.volume.display());
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
    } else {
        row("Backup drive", warning("not connected").to_string());
    }
    if mounted {
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
    if !e.nix_enabled() {
        row("Nix", "off for this drive (macup.toml)".into());
        return;
    }
    row(
        "Nix",
        find_nix()
            .map(|n| nix_version(&n))
            .unwrap_or_else(|| warning("not installed — run macup apply").to_string()),
    );
    row(
        "System",
        std::fs::read_link("/run/current-system")
            .map(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
            .unwrap_or_else(|_| warning("nix-darwin not applied yet").to_string()),
    );
    let dir = e.nix_dir().display().to_string();
    row(
        "Config",
        if !e.has_nix_config() {
            warning("missing — macup apply copies it from the drive").to_string()
        } else if !mounted {
            dir
        } else if config_in_sync(e) {
            format!("{dir} · same as drive copy")
        } else {
            format!(
                "{dir}{}",
                warning(" · differs from drive copy (saved on next apply or backup)")
            )
        },
    );
}
