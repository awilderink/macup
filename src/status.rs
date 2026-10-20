use crate::access::{full_disk_access, terminal_name};
use crate::env::Env;
use crate::ui::{human_bytes, title, warning};
use crate::volume::{check_mounted, free_space, nearly_full};

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
    row("Tools & apps", tools_and_apps(e, mounted));
}

fn tools_and_apps(e: &Env, mounted: bool) -> String {
    let install = &e.config.install;
    if !mounted {
        return warning("connect the backup drive, which lists them").to_string();
    }
    if install.is_empty() {
        return "none listed yet — `macup add <name>` adds one".into();
    }
    let mut parts: Vec<String> = install
        .lists()
        .iter()
        .map(|(kind, list)| format!("{} {}", list.len(), kind.key()))
        .collect();
    if install.rust {
        parts.push("rust".into());
    }
    let mut v = parts.join(" · ");
    if !crate::apply::installed(e, install) {
        v.push_str(&warning(" · not installed yet — run macup setup").to_string());
    }
    v
}
