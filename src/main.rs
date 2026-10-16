mod access;
mod backup;
mod config;
mod env;
mod errors;
mod guard;
mod machines;
mod menu;
mod nix;
mod packages;
mod photos;
mod picker;
mod restore;
mod rsync;
mod rules;
mod search;
mod selection;
mod setup;
mod sizer;
mod status;
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
        about = "Install whatever is missing (Command Line Tools, Nix, config) and apply the system configuration"
    )]
    Apply {
        #[arg(long, help = "update all tools to their latest versions first")]
        update: bool,
        #[arg(
            long,
            help = "only check that the configuration builds; changes nothing"
        )]
        check: bool,
    },
    #[command(
        about = "Add a command-line tool or a Mac app; macup works out which and offers to install it"
    )]
    Add {
        #[command(flatten)]
        kind: KindFlags,
        names: Vec<String>,
    },
    #[command(
        visible_alias = "rm",
        about = "Remove a command-line tool or a Mac app from your config and offer to uninstall it"
    )]
    Remove {
        #[command(flatten)]
        kind: KindFlags,
        names: Vec<String>,
    },
    #[command(about = "Open the Nix config in your editor: packages, apps or system")]
    Edit { section: Option<String> },
    #[command(about = "Drive, last backup, Photos, and tools and apps at a glance")]
    Status,
    #[command(about = "Set up a drive for macup by creating macup.toml on it")]
    Init {
        #[arg(value_name = "DRIVE", help = "the mounted drive, e.g. /Volumes/Backup")]
        drive: PathBuf,
    },
    #[command(
        about = "How much of the Photos library is on this Mac, in the originals archive or only in iCloud"
    )]
    Photos {
        #[arg(
            long,
            help = "also download one original through Photos/iCloud and compare it with the archived copy"
        )]
        test_icloud: bool,
    },
}

#[derive(clap::Args)]
struct KindFlags {
    #[arg(
        long,
        conflicts_with = "tool",
        help = "only look for a Mac app (Homebrew cask)"
    )]
    app: bool,
    #[arg(long, help = "only look for a command-line tool (nixpkgs)")]
    tool: bool,
}

impl KindFlags {
    fn forced(&self) -> Option<packages::Kind> {
        match (self.app, self.tool) {
            (true, _) => Some(packages::Kind::App),
            (_, true) => Some(packages::Kind::Tool),
            _ => None,
        }
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
        Command::Apply { .. }
        | Command::Add { .. }
        | Command::Remove { .. }
        | Command::Edit { .. }
            if !e.nix_enabled() =>
        {
            nix::require_nix(e)
        }
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
                no_photos,
                photos_only,
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
                    (true, _) => Some(restore::Mode::Overwrite),
                    (_, true) => Some(restore::Mode::OnlyMissing),
                    _ => None,
                },
                dry_run,
                yes,
                photos,
                from,
            },
        ),
        Command::Setup => setup::run(e),
        Command::Apply { update, check } => nix::apply(e, update, check),
        Command::Add { kind, names } => packages::add(e, &names, kind.forced()),
        Command::Remove { kind, names } => packages::remove(e, &names, kind.forced()),
        Command::Edit { section } => nix::edit(e, section),
        Command::Photos { test_icloud } => photos::report(e, test_icloud),
        Command::Init { drive } => config::init_drive(&drive),
        Command::Status => {
            status::show(e);
            Ok(())
        }
    }
}
