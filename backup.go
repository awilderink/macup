package main

import (
	"errors"
	"flag"
	"fmt"
	"os"
	"strings"

	"github.com/charmbracelet/huh"
)

type backupOptions struct {
	dryRun   bool
	mirror   bool
	yes      bool
	excludes []string
}

type multiFlag []string

func (m *multiFlag) String() string     { return strings.Join(*m, ",") }
func (m *multiFlag) Set(v string) error { *m = append(*m, v); return nil }

func cmdBackup(e env, args []string) error {
	fs := flag.NewFlagSet("backup", flag.ContinueOnError)
	var o backupOptions
	var excludes multiFlag
	fs.BoolVar(&o.dryRun, "dry-run", false, "show what would be copied without copying")
	fs.BoolVar(&o.dryRun, "n", false, "shorthand for --dry-run")
	fs.BoolVar(&o.mirror, "mirror", false, "also delete files from the backup that are gone from this Mac")
	fs.Var(&excludes, "exclude", "extra rsync pattern to skip (repeatable)")
	fs.BoolVar(&o.yes, "yes", false, "do not ask before deleting from the backup in --mirror mode")
	if err := fs.Parse(args); err != nil {
		return err
	}
	o.excludes = excludes
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

func backupArgs(e env, o backupOptions) []string {
	args := []string{"-ahXN", "--info=progress2"}
	if o.dryRun {
		args = append(args, "--dry-run")
	}
	if o.mirror {
		args = append(args, "--delete")
	}
	for _, x := range o.excludes {
		args = append(args, "--exclude="+x)
	}
	args = append(args, "--filter=merge "+e.rulesFile())
	return append(args, e.home+"/", e.backupHome()+"/")
}

func runBackup(e env, o backupOptions) error {
	if err := checkMounted(e.volume); err != nil {
		return err
	}
	if _, err := os.Stat(e.rulesFile()); err != nil {
		return fmt.Errorf("backup rules file missing: %s", e.rulesFile())
	}
	if o.mirror && !o.dryRun && !o.yes {
		if !interactive() {
			return errors.New("--mirror deletes files from the backup; add --yes to confirm")
		}
		yes, err := confirm("Mirror mode deletes files from the backup", "Anything no longer on this Mac will be removed from "+e.backupHome()+". Continue?", false)
		if err != nil || !yes {
			return errCancelled
		}
	}
	bin, err := findRsync(e)
	if err != nil {
		return err
	}
	l, err := acquireLock(e.lockDir())
	if err != nil {
		return err
	}
	defer l.release()
	if err := os.MkdirAll(e.backupHome(), 0o755); err != nil {
		return err
	}
	title := fmt.Sprintf("Backing up %s → %s", e.home, e.backupHome())
	if o.mirror {
		title += " (mirror)"
	}
	if o.dryRun {
		title = "Previewing backup (dry run)"
	}
	res, err := runJob(title, rsyncJob{bin: bin, args: backupArgs(e, o), logPath: logPath(e, "backup")})
	if err != nil {
		return err
	}
	if err := reportResult("Backup", res); err != nil {
		return err
	}
	if !o.dryRun {
		if err := syncNixConfigToDrive(e, bin); err != nil {
			warn("could not copy the Nix config to the drive: %v", err)
		}
	}
	if free, total, err := freeSpace(e.volume); err == nil && total > 0 && free*10 < total {
		warn("backup drive is %d%% full (%s free)", 100-free*100/total, humanBytes(int64(free)))
	}
	return nil
}
