package main

import (
	"errors"
	"flag"
	"fmt"
	"os"
	"os/exec"
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
	fs.BoolVar(&o.yes, "yes", false, "skip the safety questions (first backup from this Mac, --mirror deletions)")
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

func backupArgs(e env, o backupOptions, rules string) []string {
	args := []string{"-ahXN", "--info=progress2"}
	if o.dryRun {
		args = append(args, "--dry-run")
	}
	if o.mirror {
		args = append(args, "--delete")
	}
	return append(args, backupPaths(e, o, rules)...)
}

func backupPaths(e env, o backupOptions, rules string) []string {
	var args []string
	for _, x := range o.excludes {
		args = append(args, "--exclude="+x)
	}
	args = append(args, "--filter=merge "+rules)
	return append(args, e.home+"/", e.backupHome()+"/")
}

type deletions struct {
	count   int
	samples []string
}

func previewDeletions(e env, o backupOptions, bin, rules string) (deletions, error) {
	args := append([]string{"-a", "--dry-run", "--delete", "--out-format=%i %n"}, backupPaths(e, o, rules)...)
	out, err := exec.Command(bin, args...).Output()
	var exit *exec.ExitError
	if err != nil && (!errors.As(err, &exit) || (exit.ExitCode() != 23 && exit.ExitCode() != 24)) {
		return deletions{}, fmt.Errorf("previewing mirror deletions failed: %w", err)
	}
	var d deletions
	for _, line := range strings.Split(string(out), "\n") {
		name, found := strings.CutPrefix(line, "*deleting ")
		if !found {
			continue
		}
		d.count++
		if len(d.samples) < 8 {
			d.samples = append(d.samples, strings.TrimSpace(name))
		}
	}
	return d, nil
}

func hasBackedUpBefore(e env) bool {
	_, found := lastRealBackupLog(e)
	return found
}

func backupHasData(e env) bool {
	entries, err := os.ReadDir(e.backupHome())
	return err == nil && len(entries) > 0
}

func confirmFirstBackup(e env, o backupOptions) error {
	if o.dryRun || o.yes || hasBackedUpBefore(e) || !backupHasData(e) {
		return nil
	}
	const why = "This Mac has never been backed up with macup, but the drive already holds a backup. Backing up now replaces files in it with this Mac's versions; on a freshly installed Mac, default settings and empty app data would overwrite your old ones. Restore what you need first (macup restore)."
	if !interactive() {
		return errors.New(why + " Add --yes once you have restored.")
	}
	warn("%s", why)
	yes, err := confirm("Back up anyway?", "Choose yes only if you have restored everything you want to keep.", false)
	if err != nil || !yes {
		return errCancelled
	}
	return nil
}

func confirmMirror(e env, o backupOptions, bin, rules string) error {
	if !o.mirror || o.dryRun || o.yes {
		return nil
	}
	if !interactive() {
		return errors.New("--mirror deletes files from the backup; add --yes to confirm")
	}
	var d deletions
	var err error
	if serr := withSpinner("Checking what mirror mode would delete…", func() { d, err = previewDeletions(e, o, bin, rules) }); serr != nil {
		return serr
	}
	if err != nil {
		return err
	}
	if d.count == 0 {
		return nil
	}
	warn("mirror mode would delete %s from %s, for example:", plural(d.count, "file or folder", "files and folders"), e.backupHome())
	for _, s := range d.samples {
		note("%s", s)
	}
	yes, err := confirm(fmt.Sprintf("Delete %s from the backup?", plural(d.count, "item")), "They cannot be restored afterwards.", false)
	if err != nil || !yes {
		return errCancelled
	}
	return nil
}

func runBackup(e env, o backupOptions) error {
	if err := checkMounted(e.volume); err != nil {
		return err
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
	if err := confirmFirstBackup(e, o); err != nil {
		return err
	}
	if err := confirmMirror(e, o, bin, rules); err != nil {
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
	res, err := runJob(title, rsyncJob{bin: bin, args: backupArgs(e, o, rules), logPath: logPath(e, "backup")})
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
