# macup

One interactive tool for backing up, restoring and rebuilding this Apple Silicon Mac, written in Rust. It drives rsync for files, Nix (nix-darwin + Home Manager) for the system, and Apple's PhotoKit for originals that only exist in iCloud. Run `macup` without arguments for a menu.

The binary lives in the root of the backup drive (`/Volumes/Backup/macup`) so it works on a freshly reinstalled Mac, and in `~/.local/bin` once built here.

## After reinstalling macOS

1. Connect and unlock this backup disk.
2. Install and apply the system configuration:

   ```zsh
   /Volumes/Backup/macup apply
   ```

   It installs whatever is missing — Apple's Command Line Tools (finish that dialog and rerun), Lix (Nix), the Nix config (copied from `/Volumes/Backup/nix-darwin` to `~/.config/nix-darwin`) — then applies it: Homebrew, Ghostty, VS Code, Brave, Docker Desktop, Rekordbox, Claude Code and the command-line tools. On first run it offers to restore your zsh / Oh My Zsh config. Running it again later is safe; it only rebuilds.

3. Bring back files and folders:

   ```zsh
   /Volumes/Backup/macup restore
   ```

   Browse the backup as a tree, tick files or folders (space), open folders with →, press `p` for presets (shell, Claude, SSH, Nix config), enter to continue. You get a preview first, including files that are newer on this Mac, and choose how existing files are handled: keep newer ones (default), only add missing files, or overwrite. Nothing on the Mac is ever deleted by a restore.

## The backup drive

```
/Volumes/Backup/
├── macup                       this tool
├── macup.toml                  your settings (see Configuration)
├── README.md
├── nix-darwin/                 the Nix config a fresh Mac starts from
├── macs/
│   └── Arvins-MacBook-Pro/     one folder per Mac, named after it (MACUP_MACHINE overrides)
│       ├── home/               backup of the home folder
│       └── photos/             the Photos library copy and originals/, the archive of every original
└── archive/                    anything else you keep on the drive
```

Several Macs can share a drive; each backs up into its own folder. `macup restore` restores from this Mac's folder, or, when this Mac has none (for example after a reinstall that changed its name) and only one Mac is on the drive, from that one; `--from <name>` picks another. A drive with the older single-Mac layout (`home/` and `photos/` at the root) is moved into `macs/<this Mac>/` on the first backup or restore.

## Photos

`macup backup` also backs up `~/Pictures/*.photoslibrary` to `macs/<this Mac>/photos/` on the drive (skip it with `--no-photos`, or run only it with `--photos-only`). It keeps two things there:

- **A copy of the library** (`photos/Photos Library.photoslibrary`) that follows the Mac. The database is copied as a consistent SQLite snapshot, Photos' iCloud upload cache is left out, and the update is built on a staging copy (an APFS clone, so almost no extra space) that is only swapped in, atomically, after an integrity check and an item count. A failed or interrupted run leaves the existing copy untouched. While the Photos app is open this copy is skipped with a warning.
- **The originals archive** (`photos/originals/2024/2024-02-18/DSCF3245.JPG`, Live Photo videos next to their photo) that only ever grows. Every backup archives from the drive's library copy before and after updating it, so originals that leave the Mac — "Optimize Mac Storage", deleted photos — stay in the archive. Files are APFS clones of the library copy, so they take no extra space until the library copy drops them.

With "Optimize Mac Storage" on, a photo whose original was never downloaded to this Mac exists only in iCloud. `macup backup` notices those and offers to download them into the archive through Photos (`--fetch-icloud` does it without asking; macOS asks once for Photos access). `macup photos` shows how much of the library is on the Mac, in the archive or only in iCloud; `macup photos --test-icloud` downloads one original through Photos and checks it against the archive.

`macup restore --photos` (or the menu) copies the library back into `~/Pictures` and never overwrites one that is already there; a normal restore also skips any Photos library that already exists on the Mac.

## Day to day

| Task | Command |
| --- | --- |
| Back up the home folder | `macup backup` (`-n` previews, `--mirror` also removes files you deleted and shows how many first). The first backup from a Mac whose drive already holds a backup asks before overwriting it, so restore first. |
| Restore specific paths without the picker | `macup restore --path .ssh --path Documents/taxes` |
| Add a command-line tool or a Mac app | `macup add htop firefox` |
| Remove one (and offer to uninstall Homebrew apps) | `macup remove firefox` |
| Edit the Nix config by hand | `macup edit` |
| Rebuild after editing | `macup apply` |
| Update all packages | `macup apply --update` |
| Check the config builds without changing anything | `macup apply --check` |
| Overview | `macup status` |
| Photos coverage | `macup photos` |

`macup add` works out what each name is: a Mac app from Homebrew (`homebrew.casks`) when Homebrew has one, otherwise a command-line tool from nixpkgs (`home.packages`). When both exist it picks the app and says so; `--tool` or `--app` decides instead. It edits the list in whichever `.nix` file has it, keeping formatting and comments, and writes nothing unless every name was found; an unknown name lists the closest matches from search.nixos.org and Homebrew. `macup remove` takes a name out of `home.packages`, `homebrew.casks` or `homebrew.brews`. Homebrew is set up with `cleanup = "none"`, so removing an app from the config does not uninstall it; `macup remove` offers to, and also offers it for apps installed by hand with `brew`.

`~/.config/nix-darwin` is the config you edit; `macup apply` and `macup backup` copy it to `/Volumes/Backup/nix-darwin` so a fresh Mac can start from it.

## Configuration

macup finds the backup drive by looking for `macup.toml` at the root of a mounted volume (set `MACUP_VOLUME=/Volumes/<name>` to pick one when several are connected). `macup init /Volumes/<name>` creates it. Everything in it is optional:

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

`[nix]` is optional too. Without it, the Nix commands (`apply`, `add`, `remove`, `edit`) are available when Nix is installed or a nix-darwin config exists in `~/.config/nix-darwin` or `nix-darwin/` on the drive; `enabled = false` turns them off, and `config` points at a flake elsewhere. Without Nix, macup is a backup and restore tool: install GNU rsync with `brew install rsync`.

`backup.rules` are rsync filter rules (`- ` excludes, `+ ` includes; the first match wins). They sit between macup's built-in rules: first the exclusions of junk that is never worth backing up (caches, logs, `node_modules`, `.DS_Store`, macOS per-machine state, Photos libraries), then yours, then the built-in structure (only `~/Library/Application Support` from `~/Library`, toolchain caches like `.rustup` and `.npm`, and which parts of `~/.claude` to keep). So your `+` rules can bring back folders under `~/Library`, but not junk inside them. The built-in sets are in `rules/` in this repo. Restore presets appear under `p` in the restore picker, after the built-in shell, SSH and Nix presets. Logs go to `~/.backup-logs`.

Do not put SSH keys, Claude credentials, API keys or other secrets in the Nix config.

## Building

`./build.sh` checks formatting and lints, runs the tests, builds to `~/.local/bin/macup` and, when the backup drive is connected, copies the binary to `/Volumes/Backup/macup`. It needs Rust (`rustup`, installed by the Nix config) and Apple's Command Line Tools.

`macup apply` uses the single `darwinConfigurations` entry in `flake.nix`; with several, it picks the one named after this Mac.
