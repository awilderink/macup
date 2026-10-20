use crate::env::Env;
use crate::ui::truncate;
use anyhow::{Context, Result, anyhow, bail};
use regex::Regex;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

pub fn rsync_args<I, S>(extra: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut args = vec![
        "-ahXN".to_string(),
        "--filter=-x com.apple.macl".to_string(),
    ];
    args.extend(extra.into_iter().map(Into::into));
    args
}

pub fn dir_arg(path: &Path) -> String {
    format!("{}/", path.display().to_string().trim_end_matches('/'))
}

fn no_rsync(e: &Env) -> anyhow::Error {
    let hint = if e.nix_enabled() {
        "run `macup apply` first so it can be fetched through Nix"
    } else {
        "install it with `brew install rsync`"
    };
    anyhow!(
        "GNU rsync not found — {hint} (macOS's built-in openrsync is not reliable enough for this)"
    )
}

static RSYNC: Mutex<Option<String>> = Mutex::new(None);

pub fn find_rsync(e: &Env) -> Result<String> {
    let mut cached = RSYNC.lock().map_err(|_| anyhow!("rsync lookup poisoned"))?;
    if let Some(bin) = cached.as_ref() {
        return Ok(bin.clone());
    }
    let bin = locate_rsync(e)?;
    *cached = Some(bin.clone());
    Ok(bin)
}

fn locate_rsync(e: &Env) -> Result<String> {
    let mut candidates: Vec<PathBuf> = vec![
        "/opt/homebrew/bin/rsync".into(),
        "/usr/local/bin/rsync".into(),
        PathBuf::from("/etc/profiles/per-user")
            .join(e.user())
            .join("bin/rsync"),
        e.home.join(".nix-profile/bin/rsync"),
        "/run/current-system/sw/bin/rsync".into(),
    ];
    if let Some(found) = which("rsync")
        && !candidates.contains(&found)
    {
        candidates.insert(0, found);
    }
    if let Some(bin) = candidates.iter().find(|c| is_gnu_rsync(c)) {
        return Ok(bin.display().to_string());
    }
    let nix = crate::nix::find_nix()
        .filter(|_| e.nix_enabled())
        .ok_or_else(|| no_rsync(e))?;
    let mut args = vec!["build", "nixpkgs#rsync", "--no-link", "--print-out-paths"];
    let flake = crate::nix::prepare(e).map(|dir| dir.display().to_string());
    if let Ok(dir) = &flake {
        args.extend(["--inputs-from", dir.as_str()]);
    }
    let out = Command::new(&nix)
        .args(crate::nix::nix_args(&args))
        .output()?;
    if !out.status.success() {
        bail!(
            "fetching rsync through Nix failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let store = stdout
        .split_whitespace()
        .last()
        .ok_or_else(|| no_rsync(e))?;
    let bin = Path::new(store).join("bin/rsync");
    if !is_gnu_rsync(&bin) {
        return Err(no_rsync(e));
    }
    Ok(bin.display().to_string())
}

pub fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")?
        .to_string_lossy()
        .split(':')
        .map(|dir| Path::new(dir).join(name))
        .find(|p| p.is_file())
}

fn is_gnu_rsync(path: &Path) -> bool {
    Command::new(path)
        .arg("--version")
        .output()
        .map(|o| o.status.success() && o.stdout.starts_with(b"rsync  version"))
        .unwrap_or(false)
}

pub fn tolerable_exit(code: Option<i32>) -> bool {
    matches!(code, Some(0 | 23 | 24))
}

#[derive(Debug, Clone)]
pub struct Item {
    pub code: String,
    pub size: u64,
    pub name: String,
}

impl Item {
    pub fn deletion(&self) -> bool {
        self.code.starts_with("*deleting")
    }
    pub fn file(&self) -> bool {
        self.code.starts_with(">f")
    }
    pub fn is_new(&self) -> bool {
        self.code.starts_with(">f+++")
    }
}

pub fn parse_item(line: &str) -> Option<Item> {
    if line.len() < 12 || !line.is_char_boundary(11) {
        return None;
    }
    let (code, rest) = line.split_at(11);
    let (size, name) = rest.trim_start_matches(' ').split_once(' ')?;
    Some(Item {
        code: code.to_string(),
        size: size.parse().ok()?,
        name: name.to_string(),
    })
}

pub fn dry_run(bin: &str, args: &[String], mut each: impl FnMut(Item)) -> Result<()> {
    let mut child = Command::new(bin)
        .args(args)
        .args(["--dry-run", "--no-h", "--out-format=%i %l %n"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("starting rsync")?;
    let mut stderr = child.stderr.take().expect("stderr is piped");
    let errors = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    for line in BufReader::new(child.stdout.take().expect("stdout is piped")).lines() {
        if let Some(item) = parse_item(&line?) {
            each(item);
        }
    }
    let status = child.wait()?;
    let errors = errors.join().unwrap_or_default();
    if !tolerable_exit(status.code()) {
        bail!(
            "rsync preview failed ({status}): {}",
            truncate(errors.trim(), 300)
        );
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    pub transferred: String,
    pub percent: u64,
    pub rate: String,
    pub files: Option<String>,
}

static PROGRESS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*([\d.,]+[KMGTP]?)\s+(\d+)%\s+(\S+/s)\s+\d+:\d\d:\d\d(?:\s+\(xfr#(\d+))?")
        .expect("valid regex")
});

pub fn parse_progress(line: &str) -> Option<Progress> {
    let m = PROGRESS.captures(line)?;
    Some(Progress {
        transferred: m[1].to_string(),
        percent: m[2].parse().ok()?,
        rate: m[3].to_string(),
        files: m.get(4).map(|f| f.as_str().to_string()),
    })
}

pub struct Job {
    pub bin: String,
    pub args: Vec<String>,
    pub log_path: PathBuf,
    pub strict: bool,
}

#[derive(Debug, Clone)]
pub struct RsyncResult {
    pub exit_code: i32,
    pub duration: Duration,
    pub log_path: PathBuf,
    pub strict: bool,
}

impl RsyncResult {
    pub fn summary(&self) -> String {
        match self.exit_code {
            0 => "finished".into(),
            23 => "finished, but some files could not be copied (permissions or locked files) — see the log".into(),
            24 => "finished (some files vanished while copying, usually harmless)".into(),
            20 => "cancelled".into(),
            code => format!("failed (rsync exit {code}) — see the log"),
        }
    }

    pub fn ok(&self) -> bool {
        self.exit_code == 0 || self.exit_code == 24 || (self.exit_code == 23 && !self.strict)
    }
}

impl Job {
    pub fn run(
        &self,
        mut on_progress: impl FnMut(&Progress),
        mut on_line: impl FnMut(&str),
    ) -> Result<RsyncResult> {
        let start = Instant::now();
        if let Some(dir) = self.log_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut log = std::fs::File::create(&self.log_path)?;
        let mut args = self.args.clone();
        args.push("--info=progress2".into());
        writeln!(log, "{} {}", self.bin, args.join(" "))?;
        let (reader, writer) = std::io::pipe()?;
        let mut cmd = Command::new(&self.bin);
        cmd.args(&args).stdout(writer.try_clone()?).stderr(writer);
        let mut child = cmd.spawn().context("starting rsync")?;
        drop(cmd);
        let mut buf = Vec::new();
        for byte in BufReader::new(reader).bytes() {
            let b = byte?;
            if b != b'\r' && b != b'\n' {
                buf.push(b);
                continue;
            }
            let line = String::from_utf8_lossy(&buf).into_owned();
            buf.clear();
            if let Some(p) = parse_progress(&line) {
                on_progress(&p);
            } else if !line.trim().is_empty() {
                writeln!(log, "{line}")?;
                on_line(&line);
            }
        }
        let status = child.wait()?;
        let exit_code = status.code().unwrap_or(20);
        let res = RsyncResult {
            exit_code,
            duration: start.elapsed(),
            log_path: self.log_path.clone(),
            strict: self.strict,
        };
        writeln!(
            log,
            "{}: {} after {}s",
            chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
            res.summary(),
            res.duration.as_secs()
        )?;
        Ok(res)
    }
}

pub fn log_path(e: &Env, kind: &str) -> PathBuf {
    e.log_dir().join(format!(
        "{kind}-{}.log",
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    ))
}

pub fn write_temp(prefix: &str, data: &[u8]) -> Result<TempFile> {
    let dir = std::env::temp_dir();
    for attempt in 0..100u32 {
        let path = dir.join(format!("{prefix}-{}-{attempt}", std::process::id()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                if let Err(err) = f.write_all(data) {
                    let _ = std::fs::remove_file(&path);
                    return Err(err.into());
                }
                return Ok(TempFile(path));
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err.into()),
        }
    }
    bail!("could not create a temporary file")
}

pub struct TempFile(pub PathBuf);

impl TempFile {
    pub fn arg(&self) -> String {
        self.0.display().to_string()
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
