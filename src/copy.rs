use crate::pattern::Pattern;
use crate::rules::Filter;
use crate::selection::{at_or_under, join_rel};
use crate::ui::{human_bytes, interrupted};
use crate::volume::cstr;
use anyhow::Result;
use std::collections::HashSet;
use std::ffi::CString;
use std::fs::{File, FileTimes, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::darwin::fs::FileTimesExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

pub const TEMP_PREFIX: &str = ".macup-tmp-";
const SKIPPED_XATTRS: [&str; 1] = ["com.apple.macl"];
const CHUNK: usize = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Existing {
    #[default]
    Overwrite,
    KeepNewer,
    OnlyMissing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mirror {
    #[default]
    Off,
    Repos,
    All,
}

#[derive(Debug, Clone, Default)]
pub struct Spec {
    pub src: PathBuf,
    pub dst: PathBuf,
    pub filter: Filter,
    pub hide: Vec<Pattern>,
    pub selection: Option<Vec<String>>,
    pub existing: Existing,
    pub mirror: Mirror,
    pub skip_existing_units: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Dir {
        rel: String,
        implied: bool,
    },
    File {
        rel: String,
        size: u64,
        new: bool,
        newer_there: bool,
    },
    Link {
        rel: String,
    },
    Delete {
        rel: String,
        dir: bool,
    },
    Cleanup {
        rel: String,
    },
}

#[derive(Debug, Default)]
pub struct Plan {
    pub actions: Vec<Action>,
    pub problems: Vec<String>,
    pub specials: usize,
    pub skipped_units: Vec<String>,
}

impl Plan {
    pub fn bytes(&self) -> u64 {
        self.actions
            .iter()
            .map(|a| match a {
                Action::File { size, .. } => *size,
                _ => 0,
            })
            .sum()
    }

    pub fn files(&self) -> impl Iterator<Item = (&str, u64, bool)> {
        self.actions.iter().filter_map(|a| match a {
            Action::File { rel, size, new, .. } => Some((rel.as_str(), *size, *new)),
            _ => None,
        })
    }

    pub fn deletions(&self) -> impl Iterator<Item = &str> {
        self.actions.iter().filter_map(|a| match a {
            Action::Delete { rel, .. } => Some(rel.as_str()),
            _ => None,
        })
    }

    pub fn describe(&self) -> Vec<String> {
        self.actions
            .iter()
            .filter_map(|a| match a {
                Action::File { rel, .. } => Some(format!("copy {rel}")),
                Action::Link { rel } => Some(format!("link {rel}")),
                Action::Delete { rel, .. } => Some(format!("delete {rel}")),
                _ => None,
            })
            .collect()
    }
}

enum Reach {
    Full,
    Through,
    None,
}

impl Spec {
    fn reach(&self, rel: &str, is_dir: bool) -> Reach {
        let Some(selection) = &self.selection else {
            return Reach::Full;
        };
        if selection.iter().any(|p| at_or_under(rel, p)) {
            Reach::Full
        } else if is_dir && selection.iter().any(|p| at_or_under(p, rel)) {
            Reach::Through
        } else {
            Reach::None
        }
    }

    fn hidden(&self, rel: &str, is_dir: bool) -> bool {
        self.hide.iter().any(|p| p.matches(rel, is_dir))
    }

    fn wanted(&self, rel: &str, is_dir: bool) -> bool {
        !self.hidden(rel, is_dir) && self.filter.copies(rel, is_dir)
    }

    fn leaves_existing(&self, rel: &str, reach: &Reach) -> bool {
        if !self.skip_existing_units {
            return false;
        }
        if rel.ends_with(".photoslibrary") {
            return true;
        }
        let repo = |root: &Path| std::fs::symlink_metadata(root.join(rel).join(".git")).is_ok();
        matches!(reach, Reach::Full) && repo(&self.src) && repo(&self.dst)
    }
}

fn sorted_entries(dir: &Path) -> std::io::Result<Vec<(String, std::fs::FileType)>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(TEMP_PREFIX) {
            continue;
        }
        out.push((name, entry.file_type()?));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn seconds(t: SystemTime) -> (i64, u32) {
    match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => (d.as_secs() as i64, d.subsec_nanos()),
        Err(e) => (-(e.duration().as_secs() as i64), 0),
    }
}

fn same_time(a: SystemTime, b: SystemTime) -> bool {
    let ((sa, na), (sb, nb)) = (seconds(a), seconds(b));
    sa == sb && (na == nb || na == 0 || nb == 0)
}

pub fn plan(spec: &Spec) -> Plan {
    let mut plan = Plan::default();
    walk(spec, "", spec.mirror == Mirror::All, &mut plan);
    plan
}

fn walk(spec: &Spec, dir: &str, mirroring: bool, plan: &mut Plan) {
    let src_dir = spec.src.join(dir);
    let dst_dir = spec.dst.join(dir);
    let entries = match sorted_entries(&src_dir) {
        Ok(entries) => entries,
        Err(err) => {
            plan.problems
                .push(format!("cannot read {}: {err}", src_dir.display()));
            return;
        }
    };
    let mirroring =
        mirroring || (spec.mirror == Mirror::Repos && entries.iter().any(|(n, _)| n == ".git"));
    let mut kept: HashSet<String> = HashSet::new();
    for (name, ft) in &entries {
        let rel = join_rel(dir, name);
        let is_dir = ft.is_dir();
        let reach = spec.reach(&rel, is_dir);
        if matches!(reach, Reach::None) || !spec.wanted(&rel, is_dir) {
            continue;
        }
        kept.insert(name.clone());
        let dst = spec.dst.join(&rel);
        let there = std::fs::symlink_metadata(&dst).ok();
        if is_dir && there.is_some() && spec.leaves_existing(&rel, &reach) {
            plan.skipped_units.push(rel);
            continue;
        }
        if is_dir {
            if there.as_ref().is_some_and(|m| !m.is_dir()) {
                plan.actions.push(Action::Delete {
                    rel: rel.clone(),
                    dir: false,
                });
            }
            plan.actions.push(Action::Dir {
                rel: rel.clone(),
                implied: matches!(reach, Reach::Through),
            });
            walk(spec, &rel, mirroring, plan);
        } else if ft.is_symlink() {
            let target = std::fs::read_link(spec.src.join(&rel)).ok();
            let same = there.as_ref().is_some_and(|m| m.file_type().is_symlink())
                && std::fs::read_link(&dst).ok() == target;
            if same || (spec.existing == Existing::OnlyMissing && there.is_some()) {
                continue;
            }
            if there.as_ref().is_some_and(|m| m.is_dir()) {
                delete_tree(spec, &rel, plan, false);
            }
            plan.actions.push(Action::Link { rel });
        } else if ft.is_file() {
            let Ok(here) = std::fs::symlink_metadata(spec.src.join(&rel)) else {
                continue;
            };
            let mut newer_there = false;
            if let Some(there) = &there
                && there.is_file()
            {
                let unchanged = there.len() == here.len()
                    && matches!((here.modified(), there.modified()), (Ok(a), Ok(b)) if same_time(a, b));
                newer_there =
                    matches!((here.modified(), there.modified()), (Ok(a), Ok(b)) if b > a);
                let skip = match spec.existing {
                    Existing::OnlyMissing => true,
                    Existing::KeepNewer => unchanged || newer_there,
                    Existing::Overwrite => unchanged,
                };
                if skip {
                    continue;
                }
            } else if let Some(there) = &there {
                if spec.existing == Existing::OnlyMissing {
                    continue;
                }
                if there.is_dir() {
                    delete_tree(spec, &rel, plan, false);
                } else {
                    plan.actions.push(Action::Delete {
                        rel: rel.clone(),
                        dir: false,
                    });
                }
            }
            plan.actions.push(Action::File {
                new: there.as_ref().is_none_or(|m| !m.is_file()),
                size: here.len(),
                newer_there,
                rel,
            });
        } else {
            plan.specials += 1;
        }
    }
    let Ok(dst_names) = std::fs::read_dir(&dst_dir) else {
        return;
    };
    for entry in dst_names.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = join_rel(dir, &name);
        if name.starts_with(TEMP_PREFIX) {
            plan.actions.push(Action::Cleanup { rel });
            continue;
        }
        if !mirroring || kept.contains(&name) {
            continue;
        }
        let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
        if !spec.hidden(&rel, is_dir) && !spec.filter.copies(&rel, is_dir) {
            continue;
        }
        if is_dir {
            delete_tree(spec, &rel, plan, true);
        } else {
            plan.actions.push(Action::Delete { rel, dir: false });
        }
    }
}

fn delete_tree(spec: &Spec, rel: &str, plan: &mut Plan, protect: bool) -> bool {
    let mut emptied = true;
    if let Ok(entries) = std::fs::read_dir(spec.dst.join(rel)) {
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else {
                emptied = false;
                continue;
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            let child = join_rel(rel, &name);
            let is_dir = ft.is_dir();
            if protect
                && !name.starts_with(TEMP_PREFIX)
                && !spec.hidden(&child, is_dir)
                && !spec.filter.copies(&child, is_dir)
            {
                emptied = false;
                continue;
            }
            if is_dir {
                emptied &= delete_tree(spec, &child, plan, protect);
            } else {
                plan.actions.push(Action::Delete {
                    rel: child,
                    dir: false,
                });
            }
        }
    }
    if emptied {
        plan.actions.push(Action::Delete {
            rel: rel.to_string(),
            dir: true,
        });
    }
    emptied
}

#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    pub done: u64,
    pub percent: u64,
    pub per_second: u64,
    pub files: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Finished,
    Partial(usize),
    Vanished(usize),
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub status: Status,
    pub duration: Duration,
    pub log_path: PathBuf,
    pub strict: bool,
}

impl Outcome {
    pub fn summary(&self) -> String {
        match self.status {
            Status::Finished => "finished".into(),
            Status::Partial(n) => format!(
                "finished, but {} could not be copied (permissions or locked files) — see the log",
                crate::ui::plural(n, "file", "files")
            ),
            Status::Vanished(n) => format!(
                "finished ({} vanished while copying, usually harmless)",
                crate::ui::plural(n, "file", "files")
            ),
            Status::Cancelled => "cancelled".into(),
        }
    }

    pub fn finished(&self) -> bool {
        self.status == Status::Finished
    }

    pub fn cancelled(&self) -> bool {
        self.status == Status::Cancelled
    }

    pub fn ok(&self) -> bool {
        match self.status {
            Status::Finished | Status::Vanished(_) => true,
            Status::Partial(_) => !self.strict,
            Status::Cancelled => false,
        }
    }
}

pub struct Job {
    pub spec: Spec,
    pub log_path: PathBuf,
    pub strict: bool,
}

struct Counter<'a> {
    total: u64,
    done: u64,
    files: usize,
    start: Instant,
    last: Instant,
    report: &'a mut dyn FnMut(&Progress),
}

impl Counter<'_> {
    fn add(&mut self, bytes: u64, file_done: bool) {
        self.done += bytes;
        if file_done {
            self.files += 1;
        }
        if self.last.elapsed() >= Duration::from_millis(200) {
            self.report();
        }
    }

    fn report(&mut self) {
        self.last = Instant::now();
        let secs = self.start.elapsed().as_secs_f64().max(0.001);
        (self.report)(&Progress {
            done: self.done,
            percent: (self.done * 100).checked_div(self.total).unwrap_or(100),
            per_second: (self.done as f64 / secs) as u64,
            files: self.files,
        });
    }
}

impl Job {
    pub fn run(
        &self,
        mut on_progress: impl FnMut(&Progress),
        mut on_line: impl FnMut(&str),
    ) -> Result<Outcome> {
        let start = Instant::now();
        if let Some(dir) = self.log_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut log = File::create(&self.log_path)?;
        writeln!(
            log,
            "copying {} → {}",
            self.spec.src.display(),
            self.spec.dst.display()
        )?;
        let plan = plan(&self.spec);
        let mut counter = Counter {
            total: plan.bytes(),
            done: 0,
            files: 0,
            start,
            last: start,
            report: &mut on_progress,
        };
        let (mut failed, mut vanished) = (0, 0);
        let mut problem = |log: &mut File, line: String| -> Result<()> {
            writeln!(log, "{line}")?;
            on_line(&line);
            Ok(())
        };
        for line in &plan.problems {
            failed += 1;
            problem(&mut log, line.clone())?;
        }
        std::fs::create_dir_all(&self.spec.dst)?;
        let mut dirs: Vec<(String, bool)> = Vec::new();
        let mut cancelled = false;
        let mut buf = vec![0u8; CHUNK];
        for action in &plan.actions {
            if interrupted() {
                cancelled = true;
                break;
            }
            let mut notes = Vec::new();
            let res = match action {
                Action::Dir { rel, implied } => {
                    dirs.push((rel.clone(), *implied));
                    make_dir(&self.spec.src.join(rel), &self.spec.dst.join(rel), *implied)
                }
                Action::File { rel, .. } => copy_file(
                    &self.spec.src.join(rel),
                    &self.spec.dst.join(rel),
                    &mut buf,
                    &mut counter,
                    &mut notes,
                ),
                Action::Link { rel } => copy_link(
                    &self.spec.src.join(rel),
                    &self.spec.dst.join(rel),
                    &mut notes,
                ),
                Action::Delete { rel, dir } => remove(&self.spec.dst.join(rel), *dir),
                Action::Cleanup { rel } => remove(&self.spec.dst.join(rel), false),
            };
            if !notes.is_empty() {
                failed += 1;
                for note in notes {
                    problem(&mut log, format!("{}: {note}", describe(action)))?;
                }
            }
            match res {
                Ok(()) => {}
                Err(err) if err.kind() == ErrorKind::Interrupted && interrupted() => {
                    cancelled = true;
                    break;
                }
                Err(err)
                    if err.kind() == ErrorKind::NotFound
                        && !matches!(action, Action::Delete { .. }) =>
                {
                    vanished += 1;
                    writeln!(log, "vanished: {}", describe(action))?;
                }
                Err(err) => {
                    failed += 1;
                    problem(&mut log, format!("{}: {err}", describe(action)))?;
                }
            }
        }
        counter.report();
        for (rel, implied) in dirs.iter().rev() {
            if *implied {
                continue;
            }
            let mut notes = Vec::new();
            let res = dir_metadata(
                &self.spec.src.join(rel),
                &self.spec.dst.join(rel),
                &mut notes,
            );
            notes.extend(res.err().map(|err| err.to_string()));
            if !notes.is_empty() {
                failed += 1;
                for note in notes {
                    problem(&mut log, format!("folder {rel}: {note}"))?;
                }
            }
        }
        let status = if cancelled {
            Status::Cancelled
        } else if failed > 0 {
            Status::Partial(failed)
        } else if vanished > 0 {
            Status::Vanished(vanished)
        } else {
            Status::Finished
        };
        let outcome = Outcome {
            status,
            duration: start.elapsed(),
            log_path: self.log_path.clone(),
            strict: self.strict,
        };
        writeln!(
            log,
            "{}: {} after {}s ({} copied, {} files)",
            chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
            outcome.summary(),
            outcome.duration.as_secs(),
            human_bytes(counter.done),
            counter.files
        )?;
        Ok(outcome)
    }
}

pub fn log_path(e: &crate::env::Env, kind: &str) -> PathBuf {
    e.log_dir().join(format!(
        "{kind}-{}.log",
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    ))
}

fn describe(action: &Action) -> String {
    match action {
        Action::Dir { rel, .. } => format!("folder {rel}"),
        Action::File { rel, .. } => rel.clone(),
        Action::Link { rel } => format!("link {rel}"),
        Action::Delete { rel, .. } => format!("deleting {rel}"),
        Action::Cleanup { rel } => format!("cleaning up {rel}"),
    }
}

fn temp_path(dst: &Path) -> PathBuf {
    let name = dst
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    dst.with_file_name(format!("{TEMP_PREFIX}{}-{name}", std::process::id()))
}

fn make_dir(src: &Path, dst: &Path, implied: bool) -> std::io::Result<()> {
    let mode = std::fs::symlink_metadata(src)?.permissions().mode() & 0o7777;
    match std::fs::symlink_metadata(dst) {
        Ok(m) if m.is_dir() && implied => return Ok(()),
        Ok(m) if m.is_dir() => {}
        Ok(_) => {
            std::fs::remove_file(dst)?;
            std::fs::create_dir(dst)?;
        }
        Err(_) => std::fs::create_dir(dst)?,
    }
    std::fs::set_permissions(dst, std::fs::Permissions::from_mode(mode | 0o700))
}

fn dir_metadata(src: &Path, dst: &Path, notes: &mut Vec<String>) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(src)?;
    notes.extend(copy_xattrs(src, dst));
    let dir = File::open(dst)?;
    dir.set_times(times(&meta))?;
    std::fs::set_permissions(dst, meta.permissions())
}

fn times(meta: &std::fs::Metadata) -> FileTimes {
    let mut t = FileTimes::new();
    if let Ok(a) = meta.accessed() {
        t = t.set_accessed(a);
    }
    if let Ok(m) = meta.modified() {
        t = t.set_modified(m);
    }
    if let Ok(c) = meta.created() {
        t = t.set_created(c);
    }
    t
}

fn via_temp(
    dst: &Path,
    mut write: impl FnMut(&Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let tmp = temp_path(dst);
    let mut result = write(&tmp);
    if result
        .as_ref()
        .is_err_and(|err| err.kind() == ErrorKind::AlreadyExists)
    {
        let _ = std::fs::remove_file(&tmp);
        result = write(&tmp);
    }
    let result = result.and_then(|()| {
        if std::fs::symlink_metadata(dst).is_ok_and(|m| m.is_dir()) {
            std::fs::remove_dir_all(dst)?;
        }
        std::fs::rename(&tmp, dst)
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn copy_file(
    src: &Path,
    dst: &Path,
    buf: &mut [u8],
    counter: &mut Counter,
    notes: &mut Vec<String>,
) -> std::io::Result<()> {
    via_temp(dst, |tmp| {
        let mut from = File::open(src)?;
        let meta = from.metadata()?;
        let mut to = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(tmp)?;
        loop {
            if interrupted() {
                return Err(std::io::Error::from(ErrorKind::Interrupted));
            }
            let n = from.read(buf)?;
            if n == 0 {
                break;
            }
            to.write_all(&buf[..n])?;
            counter.add(n as u64, false);
        }
        notes.extend(copy_xattrs(src, tmp));
        to.set_permissions(meta.permissions())?;
        to.set_times(times(&meta))
    })?;
    counter.add(0, true);
    Ok(())
}

fn copy_link(src: &Path, dst: &Path, notes: &mut Vec<String>) -> std::io::Result<()> {
    let target = std::fs::read_link(src)?;
    let meta = std::fs::symlink_metadata(src)?;
    via_temp(dst, |tmp| {
        std::os::unix::fs::symlink(&target, tmp)?;
        notes.extend(copy_xattrs(src, tmp));
        let (sa, na) = seconds(meta.accessed().unwrap_or(SystemTime::UNIX_EPOCH));
        let (sm, nm) = seconds(meta.modified().unwrap_or(SystemTime::UNIX_EPOCH));
        let ts = [
            libc::timespec {
                tv_sec: sa,
                tv_nsec: na as libc::c_long,
            },
            libc::timespec {
                tv_sec: sm,
                tv_nsec: nm as libc::c_long,
            },
        ];
        let c = cstr(tmp)?;
        if unsafe {
            libc::utimensat(
                libc::AT_FDCWD,
                c.as_ptr(),
                ts.as_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    })
}

fn remove(path: &Path, dir: bool) -> std::io::Result<()> {
    let res = if dir && std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir()) {
        std::fs::remove_dir(path)
    } else {
        std::fs::remove_file(path)
    };
    match res {
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

fn xattr_names(src: &CString) -> std::io::Result<Vec<CString>> {
    let len =
        unsafe { libc::listxattr(src.as_ptr(), std::ptr::null_mut(), 0, libc::XATTR_NOFOLLOW) };
    if len < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut names = vec![0u8; len as usize];
    if len > 0 {
        let len = unsafe {
            libc::listxattr(
                src.as_ptr(),
                names.as_mut_ptr().cast(),
                names.len(),
                libc::XATTR_NOFOLLOW,
            )
        };
        if len < 0 {
            return Err(std::io::Error::last_os_error());
        }
        names.truncate(len as usize);
    }
    Ok(names
        .split(|b| *b == 0)
        .filter(|n| !n.is_empty())
        .filter(|n| !SKIPPED_XATTRS.iter().any(|skip| skip.as_bytes() == *n))
        .filter_map(|n| CString::new(n).ok())
        .collect())
}

fn copy_xattr(src: &CString, dst: &CString, name: &CString) -> std::io::Result<()> {
    let size = unsafe {
        libc::getxattr(
            src.as_ptr(),
            name.as_ptr(),
            std::ptr::null_mut(),
            0,
            0,
            libc::XATTR_NOFOLLOW,
        )
    };
    if size < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut value = vec![0u8; size as usize];
    let size = unsafe {
        libc::getxattr(
            src.as_ptr(),
            name.as_ptr(),
            value.as_mut_ptr().cast(),
            value.len(),
            0,
            libc::XATTR_NOFOLLOW,
        )
    };
    if size < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let set = unsafe {
        libc::setxattr(
            dst.as_ptr(),
            name.as_ptr(),
            value.as_ptr().cast(),
            size as usize,
            0,
            libc::XATTR_NOFOLLOW,
        )
    };
    if set != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn copy_xattrs(src: &Path, dst: &Path) -> Vec<String> {
    let (Ok(s), Ok(d)) = (cstr(src), cstr(dst)) else {
        return vec!["extended attributes: the path cannot be used".into()];
    };
    let names = match xattr_names(&s) {
        Ok(names) => names,
        Err(err) => return vec![format!("extended attributes: {err}")],
    };
    names
        .iter()
        .filter_map(|name| {
            copy_xattr(&s, &d, name)
                .err()
                .map(|err| format!("extended attribute {}: {err}", name.to_string_lossy()))
        })
        .collect()
}

#[cfg(test)]
#[path = "copy_tests.rs"]
mod tests;
