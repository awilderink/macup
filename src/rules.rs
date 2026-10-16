use crate::config::Config;
use crate::rsync::{TempFile, write_temp};
use anyhow::Result;

const JUNK: &str = include_str!("../rules/junk.rules");
const STRUCTURE: &str = include_str!("../rules/structure.rules");

pub fn compose(config: &Config) -> String {
    let mut rules = String::from(JUNK);
    for rule in &config.backup.rules {
        rules.push_str(rule);
        rules.push('\n');
    }
    rules.push_str(STRUCTURE);
    rules
}

pub fn write_rules_file(config: &Config) -> Result<TempFile> {
    write_temp("macup-backup-rules", compose(config).as_bytes())
}
