use crate::env::{Env, env_string};
use crate::guard::Guard;
use std::path::Path;

const PROTECTED_DIRS: &[&str] = &[
    "Library/Application Support/AddressBook",
    "Library/Application Support/CallHistoryDB",
    "Library/Application Support/Knowledge",
    "Library/Mail",
    "Library/Messages",
    "Library/Safari",
];

pub fn full_disk_access(home: &Path) -> Option<bool> {
    for rel in PROTECTED_DIRS {
        let dir = home.join(rel);
        if std::fs::symlink_metadata(&dir).is_err() {
            continue;
        }
        return match std::fs::read_dir(&dir).and_then(|mut entries| entries.next().transpose()) {
            Ok(_) => Some(true),
            Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => Some(false),
            Err(_) => continue,
        };
    }
    None
}

pub fn terminal_name() -> String {
    env_string("TERM_PROGRAM", "your terminal app")
}

pub fn full_disk_access_guard(e: &Env, what: &str) -> Option<Guard> {
    if full_disk_access(&e.home) != Some(false) {
        return None;
    }
    let term = terminal_name();
    Some(Guard {
        problem: format!(
            "{term} does not have Full Disk Access, so macOS blocks parts of ~/Library (Contacts, Messages, Mail, Safari, call history) and the {what} would skip them. \
             Turn it on in System Settings → Privacy & Security → Full Disk Access, then quit and reopen {term}."
        ),
        question: "Continue without Full Disk Access?".into(),
        fix: Some((
            "Open that settings page now?".into(),
            Box::new(|| {
                let _ = std::process::Command::new("open")
                    .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
                    .status();
            }),
        )),
    })
}
