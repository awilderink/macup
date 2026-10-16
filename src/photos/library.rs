use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const CORE_DATA_EPOCH: i64 = 978_307_200;

pub fn db_path(library: &Path) -> PathBuf {
    library.join("database").join("Photos.sqlite")
}

pub fn open(library: &Path) -> Result<Connection> {
    let db = db_path(library);
    let conn = Connection::open_with_flags(
        &db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("opening {}", db.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    Ok(conn)
}

pub fn item_count(library: &Path) -> Result<u64> {
    let n: i64 = open(library)?
        .query_row(
            "select count(*) from ZASSET where ZTRASHEDSTATE = 0",
            [],
            |r| r.get(0),
        )
        .context("reading the Photos database; is this a Photos library?")?;
    Ok(n as u64)
}

pub fn quick_check(library: &Path) -> Result<String> {
    Ok(open(library)?.query_row("pragma quick_check", [], |r| r.get(0))?)
}

pub fn snapshot(library: &Path, target: &Path) -> Result<()> {
    let conn = open(library)?;
    conn.execute("vacuum into ?1", [target.display().to_string()])
        .with_context(|| format!("snapshotting the Photos database into {}", target.display()))?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct Asset {
    pub original_name: Option<String>,
    pub created: Option<i64>,
}

pub fn assets(library: &Path) -> Result<HashMap<String, Asset>> {
    let conn = open(library)?;
    let mut stmt = conn
        .prepare(
            "select a.ZUUID, aa.ZORIGINALFILENAME, a.ZDATECREATED \
             from ZASSET a left join ZADDITIONALASSETATTRIBUTES aa on aa.ZASSET = a.Z_PK",
        )
        .context("this Photos database has an unknown layout (newer macOS?); the originals archive cannot name files")?;
    let rows = stmt.query_map([], |r| {
        let uuid: Option<String> = r.get(0)?;
        let name: Option<String> = r.get(1)?;
        let created: Option<f64> = r.get(2)?;
        Ok((
            uuid,
            Asset {
                original_name: name,
                created: created.map(|c| c as i64 + CORE_DATA_EPOCH),
            },
        ))
    })?;
    let mut map = HashMap::new();
    for row in rows {
        if let (Some(uuid), asset) = row? {
            map.insert(uuid, asset);
        }
    }
    if map.is_empty() && item_count(library)? > 0 {
        bail!("the Photos database lists items but no IDs; unknown layout");
    }
    Ok(map)
}

pub fn primary_files(library: &Path) -> Result<Vec<(String, String)>> {
    let conn = open(library)?;
    let mut stmt = conn
        .prepare("select ZUUID, ZDIRECTORY || '/' || ZFILENAME from ZASSET where ZTRASHEDSTATE = 0 and ZUUID is not null and ZDIRECTORY is not null and ZFILENAME is not null")
        .context("this Photos database has an unknown layout (newer macOS?)")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn uuid_of(file_name: &str) -> Option<&str> {
    let id = file_name.get(..36)?;
    let valid = id.char_indices().all(|(i, c)| {
        if [8, 13, 18, 23].contains(&i) {
            c == '-'
        } else {
            c.is_ascii_hexdigit()
        }
    });
    valid.then_some(id)
}

pub fn original_files(library: &Path) -> Result<Vec<String>> {
    let root = library.join("originals");
    let mut files = Vec::new();
    let mut stack = vec![String::new()];
    while let Some(rel) = stack.pop() {
        let dir = if rel.is_empty() {
            root.clone()
        } else {
            root.join(&rel)
        };
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound && rel.is_empty() => {
                return Ok(files);
            }
            Err(err) => return Err(err).with_context(|| format!("reading {}", dir.display())),
        };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let child = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            let ft = entry.file_type()?;
            if ft.is_dir() {
                stack.push(child);
            } else if ft.is_file() {
                files.push(child);
            }
        }
    }
    files.sort();
    Ok(files)
}
