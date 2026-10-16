package main

import (
	"bufio"
	"errors"
	"flag"
	"fmt"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"

	"github.com/charmbracelet/huh"
)

type conflictMode string

const (
	keepNewer   conflictMode = "update"
	onlyMissing conflictMode = "missing"
	overwrite   conflictMode = "overwrite"
)

func (m conflictMode) flag() []string {
	switch m {
	case keepNewer:
		return []string{"--update"}
	case onlyMissing:
		return []string{"--ignore-existing"}
	}
	return nil
}

type restorePlan struct {
	newFiles, changed, newerHere int
	newBytes, changedBytes       int64
	newerHereBytes               int64
	newerSamples                 []string
}

func (p restorePlan) bytesFor(m conflictMode) int64 {
	switch m {
	case onlyMissing:
		return p.newBytes
	case keepNewer:
		return p.newBytes + p.changedBytes - p.newerHereBytes
	}
	return p.newBytes + p.changedBytes
}

func cmdRestore(e env, args []string) error {
	fs := flag.NewFlagSet("restore", flag.ContinueOnError)
	var paths multiFlag
	fs.Var(&paths, "path", "path inside the backup to restore, e.g. .ssh or Documents/taxes (repeatable)")
	target := fs.String("target", e.home, "where to restore to")
	mode := fs.String("mode", string(keepNewer), "when a file already exists: update (keep newer local files), missing (never touch existing files), overwrite")
	dry := fs.Bool("dry-run", false, "only show what would be restored")
	fs.BoolVar(dry, "n", false, "shorthand for --dry-run")
	yes := fs.Bool("yes", false, "do not ask for confirmation")
	photos := fs.Bool("photos", false, "restore the backed-up Photos library into ~/Pictures (never overwrites one that exists)")
	if err := fs.Parse(args); err != nil {
		return err
	}
	if *photos {
		return restorePhotos(e, filepath.Join(*target, "Pictures"))
	}
	m := conflictMode(*mode)
	if m != keepNewer && m != onlyMissing && m != overwrite {
		return fmt.Errorf("unknown --mode %q", *mode)
	}
	src, err := restoreSource(e)
	if err != nil {
		return err
	}
	if len(paths) == 0 {
		if !interactive() {
			return fmt.Errorf("pass --path (repeatable) or run in a terminal to pick files")
		}
		return restoreInteractive(e, src, *target, *dry)
	}
	for i, p := range paths {
		paths[i] = strings.Trim(filepath.ToSlash(filepath.Clean(p)), "/")
		if paths[i] == "" || paths[i] == "." || strings.HasPrefix(paths[i], "..") {
			return fmt.Errorf("invalid --path %q", p)
		}
		if _, err := os.Lstat(filepath.Join(src, paths[i])); err != nil {
			return fmt.Errorf("%s is not in the backup", p)
		}
	}
	bin, err := findRsync(e)
	if err != nil {
		return err
	}
	plan, err := planWithSpinner(bin, src, *target, paths)
	if err != nil {
		return err
	}
	printPlan(plan)
	if *dry {
		return nil
	}
	if !*yes && interactive() {
		proceed, err := confirm(fmt.Sprintf("Restore %s into %s?", humanBytes(plan.bytesFor(m)), *target), "", true)
		if err != nil || !proceed {
			return errCancelled
		}
	}
	return runRestore(e, bin, src, *target, paths, m)
}

func restoreSource(e env) (string, error) {
	if _, err := os.Stat(e.backupHome()); err != nil {
		return "", fmt.Errorf("no backup at %s — is the drive connected?", e.backupHome())
	}
	return e.backupHome(), nil
}

func restoreMenu(e env) error {
	if len(backedUpLibraries(e)) > 0 {
		what := "files"
		if err := huh.NewSelect[string]().Title("What do you want to restore?").Options(
			huh.NewOption("Files and folders from the home-folder backup", "files"),
			huh.NewOption("The Photos library", "photos"),
		).Value(&what).Run(); err != nil {
			return err
		}
		if what == "photos" {
			return restorePhotos(e, filepath.Join(e.home, "Pictures"))
		}
	}
	src, err := restoreSource(e)
	if err != nil {
		return err
	}
	return restoreInteractive(e, src, e.home, false)
}

func restoreInteractive(e env, src, target string, dry bool) error {
	bin, err := findRsync(e)
	if err != nil {
		return err
	}
	paths, err := pickPaths(src)
	if err != nil {
		return err
	}
	plan, err := planWithSpinner(bin, src, target, paths)
	if err != nil {
		return err
	}
	printPlan(plan)
	if plan.newFiles+plan.changed == 0 {
		ok("Everything selected is already up to date in %s", target)
		return nil
	}
	if dry {
		return nil
	}
	m := keepNewer
	options := []huh.Option[conflictMode]{
		huh.NewOption(fmt.Sprintf("Keep files that are newer on this Mac — copies %s", humanBytes(plan.bytesFor(keepNewer))), keepNewer),
		huh.NewOption(fmt.Sprintf("Only add missing files, never touch existing ones — copies %s", humanBytes(plan.bytesFor(onlyMissing))), onlyMissing),
		huh.NewOption(fmt.Sprintf("Overwrite with the backup version — copies %s", humanBytes(plan.bytesFor(overwrite))), overwrite),
	}
	if err := huh.NewSelect[conflictMode]().Title("How should existing files be handled?").Options(options...).Value(&m).Run(); err != nil {
		return err
	}
	if m == overwrite && plan.newerHere > 0 {
		yes, err := confirm(fmt.Sprintf("Replace %d files that are newer on this Mac?", plan.newerHere), "Their newer versions will be lost.", false)
		if err != nil || !yes {
			return errCancelled
		}
	}
	return runRestore(e, bin, src, target, paths, m)
}

func writeFileList(paths []string) (string, error) {
	f, err := os.CreateTemp("", "macup-restore-*.list")
	if err != nil {
		return "", err
	}
	defer f.Close()
	for _, p := range paths {
		if _, err := f.WriteString(p + "\x00"); err != nil {
			return "", err
		}
	}
	return f.Name(), nil
}

func restoreArgs(list, src, target string, m conflictMode, extra ...string) []string {
	args := []string{"-ahXN", "-r", unreadableXattrs, "--from0", "--files-from=" + list}
	args = append(args, m.flag()...)
	args = append(args, extra...)
	return append(args, strings.TrimSuffix(src, "/")+"/", strings.TrimSuffix(target, "/")+"/")
}

func planWithSpinner(bin, src, target string, paths []string) (restorePlan, error) {
	var plan restorePlan
	var err error
	action := func() { plan, err = planRestore(bin, src, target, paths) }
	if serr := withSpinner("Comparing the backup with "+target+"…", action); serr != nil {
		return plan, serr
	}
	return plan, err
}

func planRestore(bin, src, target string, paths []string) (restorePlan, error) {
	list, err := writeFileList(paths)
	if err != nil {
		return restorePlan{}, err
	}
	defer os.Remove(list)
	args := restoreArgs(list, src, target, overwrite, "--dry-run", "--no-h", "--out-format=%i %l %n")
	cmd := exec.Command(bin, args...)
	out, err := cmd.StdoutPipe()
	if err != nil {
		return restorePlan{}, err
	}
	var stderr strings.Builder
	cmd.Stderr = &stderr
	if err := cmd.Start(); err != nil {
		return restorePlan{}, err
	}
	var plan restorePlan
	sc := bufio.NewScanner(out)
	sc.Buffer(make([]byte, 64*1024), 1024*1024)
	for sc.Scan() {
		plan.add(sc.Text(), src, target)
	}
	if err := cmd.Wait(); err != nil {
		var exit *exec.ExitError
		if !errors.As(err, &exit) || (exit.ExitCode() != 23 && exit.ExitCode() != 24) {
			return plan, fmt.Errorf("comparing failed: %v %s", err, strings.TrimSpace(stderr.String()))
		}
	}
	return plan, nil
}

func (p *restorePlan) add(line, src, target string) {
	fields := strings.SplitN(line, " ", 3)
	if len(fields) != 3 || len(fields[0]) < 2 || fields[0][1] != 'f' || fields[0][0] != '>' {
		return
	}
	size, _ := strconv.ParseInt(fields[1], 10, 64)
	name := fields[2]
	if strings.HasPrefix(fields[0][2:], "+++") {
		p.newFiles++
		p.newBytes += size
		return
	}
	p.changed++
	p.changedBytes += size
	here, err1 := os.Lstat(filepath.Join(target, name))
	there, err2 := os.Lstat(filepath.Join(src, name))
	if err1 == nil && err2 == nil && here.ModTime().After(there.ModTime()) {
		p.newerHere++
		p.newerHereBytes += size
		if len(p.newerSamples) < 8 {
			p.newerSamples = append(p.newerSamples, name)
		}
	}
}

func printPlan(p restorePlan) {
	step("Restore preview")
	note("%s (%s)", plural(p.newFiles, "new file"), humanBytes(p.newBytes))
	note("%s differ from this Mac (%s)", plural(p.changed, "file"), humanBytes(p.changedBytes))
	if p.newerHere > 0 {
		warn("%s newer on this Mac and kept unless you choose overwrite:", plural(p.newerHere, "file is", "files are"))
		for _, s := range p.newerSamples {
			note("%s", s)
		}
		if p.newerHere > len(p.newerSamples) {
			note("…and %d more", p.newerHere-len(p.newerSamples))
		}
	}
}

func runRestore(e env, bin, src, target string, paths []string, m conflictMode) error {
	list, err := writeFileList(paths)
	if err != nil {
		return err
	}
	defer os.Remove(list)
	for _, p := range paths {
		if p == "Library" || strings.HasPrefix(p, "Library/") {
			checkFullDiskAccess(e, "restore", true)
			break
		}
	}
	l, err := acquireLock(e.lockDir())
	if err != nil {
		return err
	}
	defer l.release()
	job := rsyncJob{bin: bin, args: restoreArgs(list, src, target, m, "--info=progress2"), logPath: logPath(e, "restore")}
	res, err := runJob(fmt.Sprintf("Restoring %d items into %s", len(paths), target), job)
	if err != nil {
		return err
	}
	if res.ok() {
		for _, p := range paths {
			if p == ".ssh" || strings.HasPrefix(p, ".ssh/") {
				fixSSHPermissions(filepath.Join(target, ".ssh"))
				break
			}
		}
	}
	return reportResult("Restore", res)
}

func fixSSHPermissions(dir string) {
	os.Chmod(dir, 0o700)
	filepath.WalkDir(dir, func(p string, d fs.DirEntry, err error) error {
		if err != nil || d.Type()&fs.ModeSymlink != 0 {
			return nil
		}
		switch {
		case d.IsDir():
			os.Chmod(p, 0o700)
		case strings.HasSuffix(p, ".pub"):
			os.Chmod(p, 0o644)
		default:
			os.Chmod(p, 0o600)
		}
		return nil
	})
}
