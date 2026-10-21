mod access;
mod apply;
mod backup;
mod config;
mod copy;
mod env;
mod errors;
mod guard;
mod machines;
mod menu;
mod packages;
mod pattern;
mod photos;
mod picker;
mod restore;
mod rules;
mod search;
mod selection;
mod setup;
mod sizer;
mod status;
mod tools;
mod ui;
mod volume;

use anyhow::Result;
use clap::{Parser, Subcommand};
use env::Env;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "macup",
    about = "Back up, restore and set up this Mac. Run without a command for a menu."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "Copy your home folder and Photos library to the backup drive")]
    Backup {
        #[arg(short = 'n', long, help = "show what would be copied without copying")]
        dry_run: bool,
        #[arg(
            long,
            help = "also delete files from the backup that are gone from this Mac"
        )]
        mirror: bool,
        #[arg(
            long,
            value_name = "PATTERN",
            help = "skip files matching this pattern, e.g. '*.iso' or 'Movies/' (repeatable)"
        )]
        exclude: Vec<String>,
        #[arg(long, conflicts_with = "photos_only", help = "skip the Photos library")]
        no_photos: bool,
        #[arg(long, help = "only back up the Photos library")]
        photos_only: bool,
        #[arg(
            long,
            help = "download originals that exist only in iCloud into the originals archive without asking"
        )]
        fetch_icloud: bool,
        #[arg(
            long,
            help = "continue past every safety check: no Full Disk Access, first backup from this Mac, mirror deletions"
        )]
        yes: bool,
    },
    #[command(about = "Pick files and folders to bring back from the backup")]
    Restore {
        #[arg(
            long = "path",
            value_name = "PATH",
            help = "path inside the backup to restore, e.g. .ssh or Documents/taxes (repeatable)"
        )]
        paths: Vec<String>,
        #[arg(
            long,
            value_name = "DIR",
            help = "where to restore to (default: your home folder)"
        )]
        target: Option<PathBuf>,
        #[arg(
            long,
            conflicts_with = "skip_existing",
            help = "replace files on this Mac with the backup's version, even newer ones"
        )]
        replace: bool,
        #[arg(
            long,
            help = "only add files missing on this Mac; never touch existing ones"
        )]
        skip_existing: bool,
        #[arg(short = 'n', long, help = "only show what would be restored")]
        dry_run: bool,
        #[arg(long, help = "do not ask for confirmation")]
        yes: bool,
        #[arg(
            long,
            help = "restore the backed-up Photos library into ~/Pictures (never overwrites one that exists)"
        )]
        photos: bool,
        #[arg(
            long,
            value_name = "MAC",
            help = "restore from another Mac's backup on this drive (see macup status)"
        )]
        from: Option<String>,
    },
    #[command(
        about = "Set up this Mac from the backup: tools, apps and settings, then your files and Photos"
    )]
    Setup,
    #[command(
        visible_alias = "install",
        about = "Install the tools and apps listed in macup.toml (installs Command Line Tools and Homebrew first when missing)"
    )]
    Apply {
        #[arg(
            long,
            help = "also update Homebrew and upgrade everything listed to the latest versions"
        )]
        update: bool,
        #[arg(long, help = "only show what is not installed yet; changes nothing")]
        check: bool,
    },
    #[command(
        about = "Add a tool or app; macup works out which installer has it and offers to install it"
    )]
    Add {
        #[command(flatten)]
        kind: KindFlags,
        #[arg(
            required = true,
            value_name = "NAME",
            help = "tools or apps, e.g. ripgrep firefox"
        )]
        names: Vec<String>,
    },
    #[command(
        visible_alias = "rm",
        about = "Remove a tool or app from your lists and offer to uninstall it"
    )]
    Remove {
        #[arg(
            required = true,
            value_name = "NAME",
            help = "tools or apps, e.g. ripgrep firefox"
        )]
        names: Vec<String>,
    },
    #[command(about = "Open macup.toml in your editor, then offer to apply it")]
    Edit,
    #[command(about = "Drive, last backup, Photos, and tools and apps at a glance")]
    Status {
        #[arg(
            long,
            help = "also show how much of the Photos library is on this Mac, kept on the drive or only in iCloud"
        )]
        photos: bool,
        #[arg(long, hide = true, requires = "photos")]
        test_icloud: bool,
    },
    #[command(hide = true)]
    Init {
        #[arg(value_name = "DRIVE")]
        drive: PathBuf,
    },
}

#[derive(clap::Args)]
#[group(multiple = false)]
struct KindFlags {
    #[arg(long, help = "only look for a Homebrew formula (install.brew)")]
    brew: bool,
    #[arg(long, help = "only look for a Homebrew cask (install.cask)")]
    cask: bool,
    #[arg(long, hide = true)]
    nix: bool,
    #[arg(long, hide = true)]
    app: bool,
    #[arg(long, hide = true)]
    tool: bool,
}

impl KindFlags {
    fn forced(&self) -> Result<Option<config::Kind>> {
        if self.app {
            anyhow::bail!("--app is now --cask");
        }
        if self.tool || self.nix {
            anyhow::bail!(
                "macup no longer installs from nixpkgs; use --brew for a Homebrew formula"
            );
        }
        Ok(match (self.brew, self.cask) {
            (true, _) => Some(config::Kind::Brew),
            (_, true) => Some(config::Kind::Cask),
            _ => None,
        })
    }
}

fn main() {
    ui::install_interrupt_handler();
    let cli = Cli::parse();
    let result = match cli.command {
        Some(Command::Init { drive }) => config::init_drive(&drive),
        command => Env::load().and_then(|e| match command {
            None => menu::run(e),
            Some(command) => dispatch(&e, command),
        }),
    };
    std::process::exit(errors::report(result));
}

fn dispatch(e: &Env, command: Command) -> Result<()> {
    match command {
        Command::Backup {
            dry_run,
            mirror,
            exclude,
            no_photos,
            photos_only,
            fetch_icloud,
            yes,
        } => backup::run(
            e,
            &backup::Options {
                dry_run,
                mirror,
                yes,
                photos: match (no_photos, photos_only) {
                    (true, _) => backup::Photos::Skip,
                    (_, true) => backup::Photos::Only,
                    _ => backup::Photos::Include,
                },
                fetch_icloud,
                excludes: exclude,
            },
        ),
        Command::Restore {
            paths,
            target,
            replace,
            skip_existing,
            dry_run,
            yes,
            photos,
            from,
        } => restore::run(
            e,
            restore::Request {
                paths,
                target: target.unwrap_or_else(|| e.home.clone()),
                mode: match (replace, skip_existing) {
                    (true, _) => Some(copy::Existing::Overwrite),
                    (_, true) => Some(copy::Existing::OnlyMissing),
                    _ => None,
                },
                dry_run,
                yes,
                photos,
                from,
            },
        ),
        Command::Setup => setup::run(e),
        Command::Apply { update, check } => apply::apply(e, update, check),
        Command::Add { kind, names } => packages::add(e, &names, kind.forced()?),
        Command::Remove { names } => packages::remove(e, &names),
        Command::Edit => apply::edit(e),
        Command::Init { drive } => config::init_drive(&drive),
        Command::Status {
            photos,
            test_icloud,
        } => {
            status::show(e);
            if !photos {
                return Ok(());
            }
            println!();
            photos::report(e, test_icloud)
        }
    }
}
