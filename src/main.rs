mod access;
mod backup;
mod config;
mod env;
mod errors;
mod guard;
mod machines;
mod nix;
mod packages;
mod photos;
mod picker;
mod restore;
mod rsync;
mod rules;
mod search;
mod selection;
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
            help = "extra rsync pattern to skip (repeatable)"
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
            value_name = "MODE",
            help = "when a file already exists: update (keep newer local files), missing (never touch existing files), overwrite"
        )]
        mode: Option<String>,
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
    #[command(visible_aliases = ["install", "setup"], about = "Install whatever is missing (Command Line Tools, Nix, config) and apply the system configuration")]
    Apply {
        #[arg(
            long,
            help = "update all pinned packages to their latest versions first"
        )]
        update: bool,
        #[arg(
            long,
            help = "only build the configuration to check it, without switching to it"
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
    #[command(about = "Drive, last backup, Photos, Nix and config at a glance")]
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
        Some(Command::Init { drive }) => init(&drive),
        command => Env::load().and_then(|e| match command {
            None => menu(&e),
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
            mode,
            dry_run,
            yes,
            photos,
            from,
        } => restore::run(
            e,
            restore::Request {
                paths,
                target: target.unwrap_or_else(|| e.home.clone()),
                mode: mode.as_deref().map(restore::Mode::parse).transpose()?,
                dry_run,
                yes,
                photos,
                from,
            },
        ),
        Command::Apply { update, check } => nix::apply(e, update, check),
        Command::Add { kind, names } => packages::add(e, &names, kind.forced()),
        Command::Remove { kind, names } => packages::remove(e, &names, kind.forced()),
        Command::Edit { section } => nix::edit(e, section),
        Command::Photos { test_icloud } => photos::report(e, test_icloud),
        Command::Init { drive } => init(&drive),
        Command::Status => {
            status::show(e);
            Ok(())
        }
    }
}

#[derive(Clone, Copy)]
enum Action {
    Backup,
    Restore,
    Apply,
    Update,
    Add,
    Edit,
    Quit,
}

fn menu(e: &Env) -> Result<()> {
    if !ui::interactive() {
        use clap::CommandFactory;
        Cli::command().print_help()?;
        return Ok(());
    }
    loop {
        println!();
        status::show(e);
        println!();
        let mut options = vec![
            ("Back up this Mac".to_string(), Action::Backup),
            (
                "Restore files and folders from the backup".to_string(),
                Action::Restore,
            ),
        ];
        if e.nix_enabled() {
            options.extend([
                (
                    "Install / apply the system configuration".to_string(),
                    Action::Apply,
                ),
                ("Update all packages".to_string(), Action::Update),
                ("Add a command-line tool or app".to_string(), Action::Add),
                ("Edit the configuration".to_string(), Action::Edit),
            ]);
        }
        options.push(("Quit".to_string(), Action::Quit));
        let action = match ui::select("What do you want to do?", &options) {
            Ok(Action::Quit) | Err(_) => return Ok(()),
            Ok(action) => action,
        };
        errors::report(match action {
            Action::Backup => backup::menu(e),
            Action::Restore => restore::menu(e),
            Action::Apply => nix::apply(e, false, false),
            Action::Update => nix::apply(e, true, false),
            Action::Add => add_menu(e),
            Action::Edit => nix::edit(e, None),
            Action::Quit => Ok(()),
        });
    }
}

fn add_menu(e: &Env) -> Result<()> {
    let name = ui::input("Name of the tool or app (e.g. htop or firefox)")?;
    if name.trim().is_empty() {
        return Err(errors::cancelled());
    }
    packages::add(e, &[name.trim().to_string()], None)
}

fn init(drive: &std::path::Path) -> Result<()> {
    volume::check_mounted(drive)?;
    let path = drive.join(config::FILE_NAME);
    if path.exists() {
        ui::ok(format!("{} already exists", path.display()));
    } else {
        std::fs::write(&path, config::TEMPLATE)?;
        ui::ok(format!(
            "created {}; add your own backup rules and restore presets there (see the README)",
            path.display()
        ));
    }
    Ok(())
}
