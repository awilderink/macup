use crate::access::full_disk_access_guard;
use crate::copy::{Existing, Job, Spec, log_path};
use crate::env::Env;
use crate::photos;
use crate::picker::{pick_paths, presets};
use crate::rules::Filter;
use crate::selection::is_under;
use crate::ui::{
    confirm_or_cancel, human_bytes, interactive, note, ok, plural, report_result, run_job, select,
    step, warn, with_spinner,
};
use crate::volume::Lock;
use anyhow::{Result, bail};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    KeepNewer,
    OnlyMissing,
    Overwrite,
}

impl Mode {
    fn existing(self) -> Existing {
        match self {
            Mode::KeepNewer => Existing::KeepNewer,
            Mode::OnlyMissing => Existing::OnlyMissing,
            Mode::Overwrite => Existing::Changed,
        }
    }
}

#[derive(Debug, Default)]
pub struct Plan {
    pub new_files: usize,
    pub changed: usize,
    pub newer_here: usize,
    pub new_bytes: u64,
    pub changed_bytes: u64,
    pub newer_here_bytes: u64,
    pub newer_samples: Vec<String>,
    pub skipped_libraries: Vec<String>,
}

impl Plan {
    pub fn bytes_for(&self, m: Mode) -> u64 {
        match m {
            Mode::OnlyMissing => self.new_bytes,
            Mode::KeepNewer => self.new_bytes + self.changed_bytes - self.newer_here_bytes,
            Mode::Overwrite => self.new_bytes + self.changed_bytes,
        }
    }

    fn add(&mut self, name: &str, size: u64, new: bool, src: &Path, target: &Path) {
        if let Some(root) = library_root(name) {
            if self.skipped_libraries.iter().any(|l| l == root) {
                return;
            }
            if target.join(root).exists() {
                self.skipped_libraries.push(root.to_string());
                return;
            }
        }
        if new {
            self.new_files += 1;
            self.new_bytes += size;
            return;
        }
        self.changed += 1;
        self.changed_bytes += size;
        let here = std::fs::symlink_metadata(target.join(name)).and_then(|m| m.modified());
        let there = std::fs::symlink_metadata(src.join(name)).and_then(|m| m.modified());
        if let (Ok(here), Ok(there)) = (here, there)
            && here > there
        {
            self.newer_here += 1;
            self.newer_here_bytes += size;
            if self.newer_samples.len() < 8 {
                self.newer_samples.push(name.to_string());
            }
        }
    }
}

pub fn library_root(name: &str) -> Option<&str> {
    let i = name.find(".photoslibrary/")?;
    Some(&name[..i + ".photoslibrary".len()])
}

pub struct Request {
    pub paths: Vec<String>,
    pub target: PathBuf,
    pub mode: Option<Mode>,
    pub dry_run: bool,
    pub yes: bool,
    pub photos: bool,
    pub from: Option<String>,
}

pub fn run(e: &Env, req: Request) -> Result<()> {
    let src = crate::machines::open(e, req.from.as_deref(), !req.dry_run)?;
    run_from(&src, req)
}

pub fn run_from(e: &Env, mut req: Request) -> Result<()> {
    if req.photos {
        return photos::restore_photos(e, &req.target.join("Pictures"));
    }
    require_backup(e)?;
    if req.paths.is_empty() {
        if !interactive() {
            bail!("pass --path (repeatable) or run in a terminal to pick files");
        }
        req.paths = pick_paths(&e.backup_home(), presets(&e.config))?;
    } else {
        req.mode.get_or_insert(Mode::KeepNewer);
        for p in req.paths.iter_mut() {
            let clean = clean_rel(p).ok_or_else(|| anyhow::anyhow!("invalid --path {p:?}"))?;
            if std::fs::symlink_metadata(e.backup_home().join(&clean)).is_err() {
                bail!("{p} is not in the backup");
            }
            *p = clean;
        }
    }
    restore_paths(e, &req.target, &req.paths, req.mode, req.dry_run, req.yes)
}

pub fn clean_rel(p: &str) -> Option<String> {
    let mut parts = Vec::new();
    for part in p.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            other => parts.push(other),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

pub fn require_backup(e: &Env) -> Result<()> {
    if !e.backup_home().exists() {
        bail!(
            "no backup at {} — is the drive connected?",
            e.backup_home().display()
        );
    }
    Ok(())
}

impl Request {
    pub fn pick(target: PathBuf) -> Request {
        Request {
            paths: vec![],
            target,
            mode: None,
            dry_run: false,
            yes: false,
            photos: false,
            from: None,
        }
    }
}

pub fn menu(e: &Env) -> Result<()> {
    let src = crate::machines::open(e, None, true)?;
    let photos = !photos::libraries_in(&src.photos_dir()).is_empty()
        && select(
            "What do you want to restore?",
            &[
                (
                    "Files and folders from the home-folder backup".into(),
                    false,
                ),
                ("The Photos library".into(), true),
            ],
        )?;
    run_from(
        &src,
        Request {
            photos,
            ..Request::pick(e.home.clone())
        },
    )
}

pub fn restore_paths(
    e: &Env,
    target: &Path,
    paths: &[String],
    mode: Option<Mode>,
    dry: bool,
    yes: bool,
) -> Result<()> {
    let plan = with_spinner(
        &format!("Comparing the backup with {}…", target.display()),
        || plan(e, target, paths),
    )?;
    print_plan(&plan);
    if plan.new_files + plan.changed == 0 {
        ok(format!(
            "Everything selected is already up to date in {}",
            target.display()
        ));
        return Ok(());
    }
    if dry {
        return Ok(());
    }
    let mode = match mode {
        None if plan.changed > 0 => choose_mode(&plan)?,
        m => {
            let m = m.unwrap_or(Mode::KeepNewer);
            if !yes {
                confirm_or_cancel(
                    &format!(
                        "Restore {} into {}?",
                        human_bytes(plan.bytes_for(m)),
                        target.display()
                    ),
                    true,
                )?;
            }
            m
        }
    };
    if mode == Mode::Overwrite && plan.newer_here > 0 && !yes {
        confirm_or_cancel(
            &format!(
                "Replace {} files that are newer on this Mac? Their newer versions will be lost.",
                plan.newer_here
            ),
            false,
        )?;
    }
    run_restore(e, target, paths, mode, &plan.skipped_libraries)
}

fn choose_mode(plan: &Plan) -> Result<Mode> {
    select(
        "How should existing files be handled?",
        &[
            (
                format!(
                    "Keep files that are newer on this Mac — copies {}",
                    human_bytes(plan.bytes_for(Mode::KeepNewer))
                ),
                Mode::KeepNewer,
            ),
            (
                format!(
                    "Only add missing files, never touch existing ones — copies {}",
                    human_bytes(plan.bytes_for(Mode::OnlyMissing))
                ),
                Mode::OnlyMissing,
            ),
            (
                format!(
                    "Overwrite with the backup version — copies {}",
                    human_bytes(plan.bytes_for(Mode::Overwrite))
                ),
                Mode::Overwrite,
            ),
        ],
    )
}

fn restore_spec(
    e: &Env,
    target: &Path,
    paths: &[String],
    mode: Mode,
    skip: &[String],
) -> Result<Spec> {
    let skip: Vec<String> = skip.iter().map(|lib| format!("/{lib}/")).collect();
    Ok(Spec {
        src: e.backup_home(),
        dst: target.to_path_buf(),
        filter: Filter::skipping(&skip)?,
        selection: Some(paths.to_vec()),
        existing: mode.existing(),
        ..Spec::default()
    })
}

fn plan(e: &Env, target: &Path, paths: &[String]) -> Result<Plan> {
    let spec = restore_spec(e, target, paths, Mode::Overwrite, &[])?;
    let mut plan = Plan::default();
    let src = e.backup_home();
    for (name, size, new) in crate::copy::plan(&spec).files() {
        plan.add(name, size, new, &src, target);
    }
    Ok(plan)
}

fn print_plan(p: &Plan) {
    step("Restore preview");
    for lib in &p.skipped_libraries {
        warn(format!(
            "skipping {lib}: a Photos library already exists there, and copying another library into it would damage it"
        ));
    }
    note(format!(
        "{} ({})",
        plural(p.new_files, "new file", "new files"),
        human_bytes(p.new_bytes)
    ));
    note(format!(
        "{} differ from this Mac ({})",
        plural(p.changed, "file", "files"),
        human_bytes(p.changed_bytes)
    ));
    if p.newer_here > 0 {
        warn(format!(
            "{} newer on this Mac and kept unless you choose to replace them:",
            plural(p.newer_here, "file is", "files are")
        ));
        for s in &p.newer_samples {
            note(s);
        }
        if p.newer_here > p.newer_samples.len() {
            note(format!(
                "…and {} more",
                p.newer_here - p.newer_samples.len()
            ));
        }
    }
}

fn touches(paths: &[String], dir: &str) -> bool {
    paths.iter().any(|p| p == dir || is_under(p, dir))
}

fn run_restore(
    e: &Env,
    target: &Path,
    paths: &[String],
    mode: Mode,
    skip: &[String],
) -> Result<()> {
    if touches(paths, "Library")
        && let Some(g) = full_disk_access_guard(e, "restore")
    {
        warn(&g.problem);
    }
    let _lock = Lock::acquire(&e.lock_dir())?;
    let job = Job {
        spec: restore_spec(e, target, paths, mode, skip)?,
        log_path: log_path(e, "restore"),
        strict: false,
    };
    let res = run_job(
        &format!(
            "Restoring {} into {}",
            plural(paths.len(), "item", "items"),
            target.display()
        ),
        &job,
    )?;
    if res.ok() && touches(paths, ".ssh") {
        fix_ssh_permissions(&target.join(".ssh"));
    }
    report_result("Restore", &res)
}

pub fn shell_restore(e: &Env, paths: &[String]) -> Result<()> {
    run_restore(e, &e.home, paths, Mode::KeepNewer, &[])
}

fn fix_ssh_permissions(dir: &Path) {
    let set = |p: &Path, mode: u32| {
        let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode));
    };
    set(dir, 0o700);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                set(&path, 0o700);
                stack.push(path);
            } else if path.extension().is_some_and(|x| x == "pub") {
                set(&path, 0o644);
            } else {
                set(&path, 0o600);
            }
        }
    }
}

#[cfg(test)]
#[path = "restore_tests.rs"]
mod tests;
