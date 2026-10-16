use crate::errors::cancelled;
use crate::ui::{confirm, confirm_or_cancel, interactive, warn};
use anyhow::{Result, bail};

pub struct Guard {
    pub problem: String,
    pub question: String,
    pub fix: Option<(String, Box<dyn Fn()>)>,
}

impl Guard {
    pub fn new(problem: impl Into<String>, question: impl Into<String>) -> Guard {
        Guard {
            problem: problem.into(),
            question: question.into(),
            fix: None,
        }
    }
}

pub type Check<'a> = Box<dyn FnOnce() -> Result<Option<Guard>> + 'a>;

pub fn precomputed<'a>(g: Option<Guard>) -> Check<'a> {
    Box::new(move || Ok(g))
}

pub fn resolve(checks: Vec<Check<'_>>, yes: bool) -> Result<()> {
    for check in checks {
        let Some(g) = check()? else { continue };
        warn(&g.problem);
        if yes {
            continue;
        }
        if !interactive() {
            bail!("stopped for the reason above; add --yes to continue anyway");
        }
        if let Some((prompt, fix)) = &g.fix
            && confirm(prompt, true)?
        {
            fix();
            return Err(cancelled());
        }
        confirm_or_cancel(&g.question, false)?;
    }
    Ok(())
}
