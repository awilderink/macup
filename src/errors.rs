use std::fmt;

#[derive(Debug)]
pub struct Cancelled;

impl fmt::Display for Cancelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("cancelled")
    }
}

impl std::error::Error for Cancelled {}

#[derive(Debug)]
pub struct Silent;

impl fmt::Display for Silent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("failed")
    }
}

impl std::error::Error for Silent {}

pub fn cancelled() -> anyhow::Error {
    Cancelled.into()
}

pub fn is_cancelled(err: &anyhow::Error) -> bool {
    if err.downcast_ref::<Cancelled>().is_some() {
        return true;
    }
    matches!(err.downcast_ref::<dialoguer::Error>(), Some(dialoguer::Error::IO(io)) if io.kind() == std::io::ErrorKind::Interrupted)
}

pub fn report(result: anyhow::Result<()>) -> i32 {
    let Err(err) = result else { return 0 };
    if is_cancelled(&err) {
        println!("{}", crate::ui::dim("cancelled"));
        return 130;
    }
    if err.downcast_ref::<Silent>().is_none() {
        eprintln!("{} {:#}", crate::ui::error_mark(), err);
    }
    1
}
