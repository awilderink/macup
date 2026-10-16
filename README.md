# macup

Back up your Mac to an external drive, bring it back after a reinstall, and keep your tools and apps in one place.

- **Backup:** your home folder and Photos library go to an external drive. A backup only adds and updates; it never deletes anything unless you ask.
- **Restore:** browse the backup as a tree and pick what to bring back. Files that are newer on the Mac are kept.
- **Tools and apps:** `macup add firefox ripgrep` installs a Mac app and a command-line tool and remembers both, so a fresh Mac gets them back.

macup is a single binary that lives on the backup drive itself, so it works on a Mac that has just been reinstalled. It is written in Rust for Apple Silicon Macs and uses rsync for files, Nix (nix-darwin and Home Manager) and Homebrew for tools and apps, and Apple's PhotoKit for photos that only exist in iCloud.

## Getting started

**On a freshly installed Mac,** connect the backup drive and run:

```zsh
/Volumes/Backup/macup setup
```

It walks you through three steps, each of which you can skip:

1. **Tools, apps and settings.** It installs what is missing (Apple's Command Line Tools, Nix, your config from the drive) and applies it. If the Command Line Tools installer appears, finish it and run `macup setup` again.
2. **Files and folders.** Pick what to bring back from the backup. You see a preview first, including any files that are newer on this Mac.
3. **Photos.** It shows the size of your backed-up Photos library next to the Mac's free space and asks before copying it back.

**Any other time,** just run `macup`. It shows where things stand and puts the most useful next step first: a backup when the last one is a day old, a restore before the first backup from a new Mac, or setting up a newly connected drive.

## Everyday commands

| What | Command |
| --- | --- |
| Back up this Mac | `macup backup` |
| Bring back files and folders | `macup restore` |
| Add a tool or app | `macup add htop firefox` |
| Remove one | `macup remove firefox` |
| See how things stand | `macup status` |
| Apply changes to your tools and settings | `macup apply` |
| Edit your config by hand | `macup edit` |

Every command has `--help`.

## Backing up

`macup backup` copies your home folder to the drive, then your Photos library (see [Photos](#photos)).

- **Only what matters is copied.** Caches, logs, `node_modules`, toolchains that install themselves again, and per-machine macOS state are skipped. From `~/Library` only `Application Support` is kept. You can add your own rules (see [Settings](#settings)).
- **Nothing is deleted.** Files you deleted on the Mac stay in the backup. To remove them as well, use `macup backup --mirror` (in the menu: "Clean up the backup"). It first shows how many files would go and asks.
- **First backup from a new Mac:** if the drive already holds a backup for this Mac, macup warns first. A fresh Mac's default settings would otherwise replace your old ones, so restore what you need before backing up.
- **Full Disk Access:** give your terminal Full Disk Access in System Settings → Privacy & Security, or protected folders in `~/Library` are left out. `macup status` tells you whether it has it.
- **Preview:** `macup backup -n` shows what would be copied without copying anything.

## Restoring

`macup restore` opens the backup as a tree. Space ticks a file or folder, → opens a folder, `p` offers presets (shell config, SSH keys, Nix config and your own), and Enter continues.

- **You see a preview first:** how much will be copied and which files are newer on this Mac.
- **Newer files on the Mac are kept.** A restore never deletes anything. If files differ, you choose: keep the newer ones, only add missing files, or take the backup's version.
- **Without the picker:** `macup restore --path .ssh --path Documents/taxes`, with `--replace` to take the backup's version anyway or `--skip-existing` to only add missing files.
- **SSH keys** get the right permissions again after a restore.
- **Several Macs on one drive:** macup restores from this Mac's backup. If there is none, for example after a reinstall that changed the Mac's name, it uses the only Mac on the drive. `--from <name>` picks another.

## Photos

`macup backup` also backs up every Photos library in `~/Pictures`. The drive keeps two things:

- **A copy of the library** that matches the Mac. It is updated safely: the new version is built next to the old one and only swapped in once it has been checked, so an interrupted backup leaves the old copy intact. While the Photos app is open this part is skipped; quit Photos and run `macup backup --photos-only`.
- **An archive of every original** (`photos/originals/2024/2024-02-18/DSCF3245.JPG`), which only ever grows. Originals that leave the Mac, because you deleted them or because Photos' "Optimize Mac Storage" removed them, stay in the archive. It takes almost no extra space on the drive.

With "Optimize Mac Storage" on, some originals exist only in iCloud. `macup backup` notices them and offers to download them into the archive; `--fetch-icloud` does it without asking. `macup status --photos` shows how many originals are on the Mac, in the archive and only in iCloud.

`macup restore --photos` puts the library back into `~/Pictures`. It never overwrites a library that is already there, and a normal restore skips Photos libraries too.

## Tools and apps

Your tools and apps are listed in a Nix config in `~/.config/nix-darwin`. `macup apply` installs everything on those lists. Each apply and backup also saves the config to the drive, so a fresh Mac can start from it.

- **`macup add <name>`** works out what a name is. When Homebrew has a Mac app by that name, it adds the app; otherwise it adds the command-line tool from nixpkgs. If both exist it picks the app and says so; `--tool` or `--app` decides instead. When a name is not found, you get the closest matches from search.nixos.org and Homebrew. Nothing is written unless every name was found, and your config's formatting and comments are kept.
- **`macup remove <name>`** finds the name in your config and takes it out. For apps it also offers to uninstall them, because Homebrew is set up to leave unlisted apps alone. That also works for apps you installed by hand with `brew`.
- **`macup apply --update`** updates everything to the latest versions; `--check` only checks that the config builds.
- **`macup edit`** opens the config in your editor: `packages` (command-line tools), `apps` (Homebrew apps) or `system` (macOS and Nix settings).

Without Nix, macup is a backup and restore tool; install GNU rsync with `brew install rsync`.

## The backup drive

```
/Volumes/Backup/
├── macup              this tool
├── macup.toml         settings for this drive
├── nix-darwin/        your Nix config, for a fresh Mac
└── macs/
    └── <Mac name>/    one folder per Mac
        ├── home/      the home folder backup
        └── photos/    the Photos library copy and the originals archive
```

macup recognises its drive by the `macup.toml` at the top. When you connect a drive that doesn't have one, `macup` offers to set it up. Several Macs can share one drive; each backs up into a folder named after it.

## Settings

`macup.toml` on the drive holds your own rules and presets. Everything in it is optional:

```toml
[backup]
rules = [
  "- /go",
  "- /code/**/target/",
  "+ /Library/Pioneer/***",
]

[[restore.presets]]
name = "Claude Code config"
paths = [".claude"]

[nix]
enabled = true
config = "~/.config/nix-darwin"
```

- **`backup.rules`** are rsync filter rules: `- ` skips, `+ ` includes, and the first match wins. Paths starting with `/` are relative to your home folder. Your rules come after macup's built-in skips for caches and junk and before its built-in structure, so a `+` rule can bring back a folder under `~/Library`, but not the caches inside it. The built-in rules are in [`rules/`](rules).
- **`restore.presets`** appear under `p` in the restore picker.
- **`[nix]`:** without it, the Nix commands are available whenever Nix is installed or a config exists. `enabled = false` turns them off, and `config` points at a config somewhere else.

Never put SSH keys, API keys or other secrets in the Nix config.

Environment variables, mostly for testing:

| Variable | Use |
| --- | --- |
| `MACUP_VOLUME` | which drive to use, when several are connected |
| `MACUP_MACHINE` | the name of this Mac's folder on the drive |
| `MACUP_HOME` | the home folder to back up |

Logs of every backup and restore are in `~/.backup-logs`.

## Building

```zsh
./build.sh
```

This checks formatting and lints, runs the tests, builds `~/.local/bin/macup` and, when the backup drive is connected, copies the binary to the drive. It needs Rust and Apple's Command Line Tools; both come with the Nix config.
