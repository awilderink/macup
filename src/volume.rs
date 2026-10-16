use anyhow::{Context, Result, bail};
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub fn check_mounted(volume: &Path) -> Result<()> {
    let vol = std::fs::metadata(volume).with_context(|| {
        format!(
            "{} not found — is the backup drive connected?",
            volume.display()
        )
    })?;
    let parent = std::fs::metadata(volume.parent().unwrap_or(Path::new("/")))?;
    if vol.dev() == parent.dev() {
        bail!(
            "{} is an empty folder, not the mounted drive — connect and unlock the backup disk",
            volume.display()
        );
    }
    Ok(())
}

pub fn free_space(path: &Path) -> Result<(u64, u64)> {
    let c = cstr(path)?;
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let bsize = st.f_bsize as u64;
    Ok((st.f_bavail * bsize, st.f_blocks * bsize))
}

pub fn unprepared_drives(volumes: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(volumes) else {
        return Vec::new();
    };
    let mut drives: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|ft| ft.is_dir()))
        .map(|entry| entry.path())
        .filter(|p| !p.join(crate::config::FILE_NAME).exists())
        .filter(|p| check_mounted(p).is_ok() && writable_and_browsable(p))
        .filter(|p| !is_installer(p))
        .collect();
    drives.sort();
    drives
}

fn writable_and_browsable(path: &Path) -> bool {
    let Ok(c) = cstr(path) else { return false };
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return false;
    }
    let hidden = (libc::MNT_RDONLY | libc::MNT_DONTBROWSE) as u32;
    st.f_flags & hidden == 0
}

fn is_installer(path: &Path) -> bool {
    std::fs::read_dir(path).is_ok_and(|entries| {
        entries.flatten().any(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.starts_with("Install macOS") && name.ends_with(".app")
        })
    })
}

pub fn disk_usage(path: &Path) -> Option<u64> {
    let out = std::process::Command::new("du")
        .arg("-sk")
        .arg(path)
        .output()
        .ok()?;
    let kib: u64 = String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    Some(kib * 1024)
}

pub fn nearly_full(free: u64, total: u64) -> bool {
    total > 0 && free * 10 < total
}

pub fn cstr(path: &Path) -> Result<CString> {
    Ok(CString::new(path.as_os_str().as_bytes())?)
}

pub fn clonefile(src: &Path, dst: &Path) -> Result<()> {
    let (s, d) = (cstr(src)?, cstr(dst)?);
    if unsafe { libc::clonefile(s.as_ptr(), d.as_ptr(), 0x0001) } != 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("cloning {} to {}", src.display(), dst.display()));
    }
    Ok(())
}

pub fn rename_swap(a: &Path, b: &Path) -> Result<()> {
    let (x, y) = (cstr(a)?, cstr(b)?);
    if unsafe { libc::renamex_np(x.as_ptr(), y.as_ptr(), libc::RENAME_SWAP) } != 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("swapping {} and {}", a.display(), b.display()));
    }
    Ok(())
}

pub struct Lock {
    dir: PathBuf,
}

impl Lock {
    pub fn acquire(dir: &Path) -> Result<Lock> {
        if std::fs::create_dir(dir).is_err() {
            let pid: i32 = std::fs::read_to_string(dir.join("pid"))
                .ok()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0);
            if pid > 0 && process_alive(pid) {
                bail!("another backup or restore is running (pid {pid})");
            }
            std::fs::remove_dir_all(dir)?;
            std::fs::create_dir(dir)?;
        }
        if let Err(err) = std::fs::write(dir.join("pid"), format!("{}\n", std::process::id())) {
            let _ = std::fs::remove_dir_all(dir);
            return Err(err.into());
        }
        Ok(Lock {
            dir: dir.to_path_buf(),
        })
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn process_alive(pid: i32) -> bool {
    unsafe {
        libc::kill(pid, 0) == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}
