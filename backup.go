package main

import (
	"errors"
	"flag"
	"fmt"
	"os"
	"strings"
	"time"

	"github.com/charmbracelet/huh"
)

type backupOptions struct {
	dryRun     bool
	mirror     bool
	yes        bool
	noPhotos   bool
	photosOnly bool
	excludes   []string
}

func cmdBackup(e env, args []string) error {
	fs := flag.NewFlagSet("backup", flag.ContinueOnError)
	var o backupOptions
	fs.BoolVar(&o.dryRun, "dry-run", false, "show what would be copied without copying")
	fs.BoolVar(&o.dryRun, "n", false, "shorthand for --dry-run")
	fs.BoolVar(&o.mirror, "mirror", false, "also delete files from the backup that are gone from this Mac")
	fs.Func("exclude", "extra rsync pattern to skip (repeatable)", func(s string) error {
		o.excludes = append(o.excludes, s)
		return nil
	})
	fs.BoolVar(&o.noPhotos, "no-photos", false, "skip the Photos library")
	fs.BoolVar(&o.photosOnly, "photos-only", false, "only back up the Photos library")
	fs.BoolVar(&o.yes, "yes", false, "continue past every safety check: no Full Disk Access, first backup from this Mac, mirror deletions, a Photos library that shrank")
	if err := fs.Parse(args); err != nil {
		return err
	}
	return runBackup(e, o)
}

func backupMenu(e env) error {
	mode := "add"
	dry := false
	form := huh.NewForm(huh.NewGroup(
		huh.NewSelect[string]().Title("How should the backup treat files you deleted on this Mac?").
			Options(
				huh.NewOption("Keep them in the backup (safe, recommended)", "add"),
				huh.NewOption("Delete them from the backup too (mirror)", "mirror"),
			).Value(&mode),
		huh.NewConfirm().Title("Preview only?").Description("A dry run shows what would change without copying anything.").
			Affirmative("Preview").Negative("Back up for real").Value(&dry),
	))
	if err := form.Run(); err != nil {
		return err
	}
	return runBackup(e, backupOptions{dryRun: dry, mirror: mode == "mirror"})
}

func backupArgs(e env, o backupOptions, rules string) []string {
	args := rsyncArgs()
	if o.mirror {
		args = append(args, "--delete")
	}
	for _, x := range o.excludes {
		args = append(args, "--exclude="+x)
	}
	return append(args, "--filter=merge "+rules, e.home+"/", e.backupHome()+"/")
}

func hasBackedUpBefore(e env) bool {
	_, err := os.Stat(e.lastBackupFile())
	return err == nil
}

func firstBackupGuard(e env) *guard {
	if hasBackedUpBefore(e) {
		return nil
	}
	if entries, err := os.ReadDir(e.backupHome()); err != nil || len(entries) == 0 {
		return nil
	}
	return &guard{
		problem:  "This Mac has never been backed up with macup, but the drive already holds a backup. Backing up now replaces files in it with this Mac's versions; on a freshly installed Mac, default settings and empty app data would overwrite your old ones. Restore what you need first (macup restore).",
		question: "Back up anyway? Only if you have restored everything you want to keep.",
	}
}

func mirrorGuard(e env, o backupOptions, bin, rules string) (*guard, error) {
	var count int
	var samples []string
	err := withSpinner("Checking what mirror mode would delete…", func() error {
		return dryRun(bin, append(backupArgs(e, o, rules), "--existing", "--ignore-existing"), func(it rsyncItem) {
			if !it.deletion() {
				return
			}
			count++
			if len(samples) < 8 {
				samples = append(samples, it.name)
			}
		})
	})
	if err != nil || count == 0 {
		return nil, err
	}
	return &guard{
		problem:  fmt.Sprintf("Mirror mode would delete %s from %s, for example:\n    %s", plural(count, "file or folder", "files and folders"), e.backupHome(), strings.Join(samples, "\n    ")),
		question: fmt.Sprintf("Delete %s from the backup? They cannot be restored afterwards.", plural(count, "item")),
	}, nil
}

func runBackup(e env, o backupOptions) error {
	if err := checkMounted(e.volume); err != nil {
		return err
	}
	if o.noPhotos && o.photosOnly {
		return errors.New("--no-photos and --photos-only cannot be combined")
	}
	bin, err := findRsync(e)
	if err != nil {
		return err
	}
	rules, err := writeRulesFile()
	if err != nil {
		return err
	}
	defer os.Remove(rules)
	var libs []photoLibrary
	if !o.noPhotos {
		libs = photoLibraries(e)
	}
	if len(libs) > 0 && !o.dryRun && photosAppRunning() {
		return errors.New("quit the Photos app first; its library is a live database that cannot be copied safely while it is open")
	}
	checks := []check{precomputed(fullDiskAccessGuard(e, "backup"))}
	if !o.photosOnly {
		checks = append(checks, precomputed(firstBackupGuard(e)))
		if o.mirror {
			checks = append(checks, func() (*guard, error) { return mirrorGuard(e, o, bin, rules) })
		}
	}
	for _, l := range libs {
		checks = append(checks, l.shrinkGuard)
	}
	if err := resolveGuards(checks, o.yes || o.dryRun); err != nil {
		return err
	}
	l, err := acquireLock(e.lockDir())
	if err != nil {
		return err
	}
	defer l.release()
	if !o.photosOnly {
		if err := backupHomeFolder(e, o, bin, rules); err != nil {
			return err
		}
	}
	for _, lib := range libs {
		if err := backupPhotoLibrary(e, o, bin, lib); err != nil {
			return err
		}
	}
	if free, total, err := freeSpace(e.volume); err == nil && nearlyFull(free, total) {
		warn("backup drive is %d%% full (%s free)", 100-free*100/total, humanBytes(int64(free)))
	}
	return nil
}

func backupHomeFolder(e env, o backupOptions, bin, rules string) error {
	if err := os.MkdirAll(e.backupHome(), 0o755); err != nil {
		return err
	}
	title := fmt.Sprintf("Backing up %s → %s", e.home, e.backupHome())
	args := backupArgs(e, o, rules)
	switch {
	case o.dryRun:
		title = "Previewing backup (dry run)"
		args = append(args, "--dry-run")
	case o.mirror:
		title += " (mirror)"
	}
	res, err := runJob(title, rsyncJob{bin: bin, args: args, logPath: logPath(e, "backup")})
	if err != nil {
		return err
	}
	if err := reportResult("Backup", res); err != nil || o.dryRun {
		return err
	}
	stamp := fmt.Sprintf("%s · %s\n", time.Now().Format("2006-01-02 15:04"), res.summary())
	if err := os.WriteFile(e.lastBackupFile(), []byte(stamp), 0o644); err != nil {
		warn("could not record the backup time: %v", err)
	}
	if err := syncNixConfigToDrive(e, bin); err != nil {
		warn("could not copy the Nix config to the drive: %v", err)
	}
	return nil
}

func lastBackup(e env) string {
	if data, err := os.ReadFile(e.lastBackupFile()); err == nil {
		return strings.TrimSpace(string(data))
	}
	if info, err := os.Stat(e.backupHome()); err == nil {
		return "never from this Mac (backup folder last changed " + info.ModTime().Format("2006-01-02 15:04") + ")"
	}
	return "none"
}
