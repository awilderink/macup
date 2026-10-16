use crate::rsync::{TempFile, write_temp};
use anyhow::Result;

pub const BACKUP_RULES: &[u8] = include_bytes!("../backup.rules");

pub fn write_rules_file() -> Result<TempFile> {
    write_temp("macup-backup-rules", BACKUP_RULES)
}
