# macup

One interactive tool for backing up, restoring and rebuilding this Apple Silicon Mac. It drives rsync for files and Nix (nix-darwin + Home Manager) for the system. Run `macup` without arguments for a menu.

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
├── macup          this tool
├── README.md
├── home/          backup of the home folder
├── photos/        backup of the Photos library
├── nix-darwin/    the Nix config a fresh Mac starts from
└── archive/       PhotoRescue, pre-wipe files, the old shell scripts
```

## Photos

`macup backup` also backs up `~/Pictures/*.photoslibrary` to `photos/` on the drive (skip it with `--no-photos`, or run only it with `--photos-only`). A Photos library is a live database, so:

- Quit the Photos app first; the backup refuses while it is open.
- The database is copied as a consistent SQLite snapshot, then the rest of the library is synced around it. Photos' iCloud upload cache (`resources/cpl`) is left out.
- The previous copy is kept aside as an APFS clone (almost no extra space) until the new copy passes an integrity check and has the same number of items; otherwise the previous copy is put back. An interrupted run is rolled back on the next one.
- If the library on this Mac has fewer items or original files than the backup — for example because "Optimize Mac Storage" left originals only in iCloud — it refuses unless you pass `--yes`.

`macup restore --photos` (or the menu) copies the library back into `~/Pictures`, and never overwrites one that is already there.

## Day to day

| Task | Command |
| --- | --- |
| Back up the home folder | `macup backup` (`-n` previews, `--mirror` also removes files you deleted and shows how many first). The first backup from a Mac whose drive already holds a backup asks before overwriting it, so restore first. |
| Restore specific paths without the picker | `macup restore --path .ssh --path Documents/taxes` |
| Add / remove a command-line tool | `macup add htop`, `macup remove htop` |
| Add a Mac app from Homebrew | `macup add --app firefox` |
| Edit the Nix config by hand | `macup edit` |
| Rebuild after editing | `macup apply` |
| Update all packages | `macup apply --update` |
| Check the config builds without changing anything | `macup apply --check` |
| Overview | `macup status` |

`~/.config/nix-darwin` is the config you edit; `macup apply` and `macup backup` copy it to `/Volumes/Backup/nix-darwin` so a fresh Mac can start from it.

What the backup skips (caches, `node_modules`, most of `~/Library`, Claude session history, …) is listed in `backup.rules`, in rsync filter syntax; the first matching rule wins. The rules are compiled into the binary, so rebuild after editing them. Photos libraries are skipped: the backup drive already holds the old library and has no room for the current one next to it. Logs go to `~/.backup-logs`.

Do not put SSH keys, Claude credentials, API keys or other secrets in the Nix config.

## Building

`./build.sh` runs the tests, builds to `~/.local/bin/macup` and, when the backup drive is connected, copies the binary to `/Volumes/Backup/macup`. It only needs Nix, which provides Go.

For a different Mac or hostname, change `Arvins-MacBook-Pro` in `nix-darwin/flake.nix`, or set `MACUP_HOST`.
