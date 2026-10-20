use crate::copy::{Job, Outcome, Progress};
use crate::errors::{Silent, cancelled};
use anyhow::Result;
use console::{Style, StyledObject, Term};
use indicatif::{ProgressBar, ProgressStyle};
use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static JOB_RUNNING: AtomicBool = AtomicBool::new(false);
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

pub fn install_interrupt_handler() {
    let _ = ctrlc::set_handler(|| {
        if JOB_RUNNING.load(Ordering::SeqCst) {
            INTERRUPTED.store(true, Ordering::SeqCst);
            return;
        }
        let _ = Term::stdout().show_cursor();
        let _ = Term::stderr().show_cursor();
        println!();
        std::process::exit(130);
    });
}

fn accent() -> Style {
    Style::new().magenta().bold()
}

pub fn title<D>(s: D) -> StyledObject<D> {
    accent().apply_to(s)
}

pub fn dim<D>(s: D) -> StyledObject<D> {
    Style::new().dim().apply_to(s)
}

pub fn warning<D>(s: D) -> StyledObject<D> {
    Style::new().yellow().apply_to(s)
}

pub fn error_mark() -> StyledObject<&'static str> {
    Style::new().red().bold().apply_to("✗")
}

pub fn interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

pub fn step(msg: impl AsRef<str>) {
    println!("{} {}", title("==>"), msg.as_ref());
}

pub fn ok(msg: impl AsRef<str>) {
    println!("{} {}", Style::new().green().apply_to("✓"), msg.as_ref());
}

pub fn warn(msg: impl AsRef<str>) {
    println!("{} {}", warning("!"), msg.as_ref());
}

pub fn note(msg: impl AsRef<str>) {
    println!("{}", dim(format!("  {}", msg.as_ref())));
}

pub fn confirm(prompt: &str, default: bool) -> Result<bool> {
    if !interactive() {
        return Ok(default);
    }
    match dialoguer::Confirm::new()
        .with_prompt(prompt)
        .default(default)
        .interact_opt()?
    {
        Some(answer) => Ok(answer),
        None => Err(cancelled()),
    }
}

pub fn confirm_or_cancel(prompt: &str, default: bool) -> Result<()> {
    if confirm(prompt, default)? {
        Ok(())
    } else {
        Err(cancelled())
    }
}

pub fn select<T: Clone>(prompt: &str, options: &[(String, T)]) -> Result<T> {
    let labels: Vec<&str> = options.iter().map(|(l, _)| l.as_str()).collect();
    match dialoguer::Select::new()
        .with_prompt(prompt)
        .items(&labels)
        .default(0)
        .interact_opt()?
    {
        Some(i) => Ok(options[i].1.clone()),
        None => Err(cancelled()),
    }
}

pub fn input(prompt: &str) -> Result<String> {
    Ok(dialoguer::Input::<String>::new()
        .with_prompt(prompt)
        .allow_empty(true)
        .interact_text()?)
}

pub fn with_spinner<T>(message: &str, action: impl FnOnce() -> Result<T>) -> Result<T> {
    if !interactive() {
        return action();
    }
    let pb = ProgressBar::new_spinner().with_message(message.to_string());
    pb.enable_steady_tick(Duration::from_millis(100));
    let result = action();
    pb.finish_and_clear();
    result
}

pub fn run_job(heading: &str, job: &Job) -> Result<Outcome> {
    INTERRUPTED.store(false, Ordering::SeqCst);
    JOB_RUNNING.store(true, Ordering::SeqCst);
    let result = if interactive() {
        run_job_with_bar(heading, job)
    } else {
        run_job_plain(heading, job)
    };
    JOB_RUNNING.store(false, Ordering::SeqCst);
    result
}

fn run_job_plain(heading: &str, job: &Job) -> Result<Outcome> {
    step(heading);
    let mut last = Instant::now() - Duration::from_secs(10);
    job.run(
        |p: &Progress| {
            if last.elapsed() >= Duration::from_secs(5) {
                last = Instant::now();
                println!("  {}  {}%  {}", p.transferred, p.percent, p.rate);
            }
        },
        |line| println!("  {line}"),
    )
}

fn run_job_with_bar(heading: &str, job: &Job) -> Result<Outcome> {
    let pb = ProgressBar::new(100).with_message(heading.to_string());
    pb.set_style(
        ProgressStyle::with_template("{spinner} {msg}\n  {wide_bar} {pos:>3}%\n  {prefix}")
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("━━─"),
    );
    pb.set_prefix(dim("comparing files…  ctrl+c stops safely").to_string());
    pb.enable_steady_tick(Duration::from_millis(100));
    let start = Instant::now();
    let mut shown = 0;
    let mut messages = 0;
    let result = job.run(
        |p: &Progress| {
            pb.set_position(p.percent);
            let mut info = format!("{} copied · {}", p.transferred, p.rate);
            if let Some(files) = &p.files {
                info.push_str(&format!(" · {files} files"));
            }
            info.push_str(&format!(
                " · {}s elapsed · ctrl+c stops safely",
                start.elapsed().as_secs()
            ));
            pb.set_prefix(dim(info).to_string());
        },
        |line| {
            messages += 1;
            if shown < 5 {
                shown += 1;
                pb.println(format!("  {}", dim(truncate(line, 110))));
            }
        },
    );
    pb.finish_and_clear();
    if messages > shown {
        warn(format!(
            "{messages} files had problems; the full list is in the log"
        ));
    }
    result
}

pub fn report_result(what: &str, res: &Outcome) -> Result<()> {
    let line = format!("{what} {} in {}s", res.summary(), res.duration.as_secs());
    if res.finished() {
        ok(&line);
    } else if res.ok() {
        warn(&line);
    } else {
        println!("{} {line}", error_mark());
    }
    note(format!("log: {}", res.log_path.display()));
    if res.cancelled() {
        return Err(cancelled());
    }
    if !res.ok() {
        return Err(Silent.into());
    }
    Ok(())
}

pub fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
    out.push('…');
    out
}

pub fn human_bytes(n: u64) -> String {
    const UNIT: u64 = 1000;
    if n < UNIT {
        return format!("{n} B");
    }
    let (mut div, mut exp) = (UNIT, 0);
    let mut m = n / UNIT;
    while m >= UNIT {
        div *= UNIT;
        exp += 1;
        m /= UNIT;
    }
    format!(
        "{:.1} {}B",
        n as f64 / div as f64,
        ['k', 'M', 'G', 'T', 'P', 'E'][exp]
    )
}

pub fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}
