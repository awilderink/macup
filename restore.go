package main

import (
	"flag"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"slices"
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
	skippedLibraries             []string
}

func libraryRoot(name string) (string, bool) {
	i := strings.Index(name, ".photoslibrary/")
	if i < 0 {
		return "", false
	}
	return name[:i+len(".photoslibrary")], true
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
	var paths []string
	fs.Func("path", "path inside the backup to restore, e.g. .ssh or Documents/taxes (repeatable)", func(s string) error {
		paths = append(paths, s)
		return nil
	})
	target := fs.String("target", e.home, "where to restore to")
	mode := fs.String("mode", "", "when a file already exists: update (keep newer local files, the default with --path), missing (never touch existing files), overwrite")
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
	if m != "" && m != keepNewer && m != onlyMissing && m != overwrite {
		return fmt.Errorf("unknown --mode %q", *mode)
	}
	if err := requireBackup(e); err != nil {
		return err
	}
	if len(paths) == 0 {
		if !interactive() {
			return fmt.Errorf("pass --path (repeatable) or run in a terminal to pick files")
		}
		picked, err := pickPaths(e.backupHome())
		if err != nil {
			return err
		}
		paths = picked
	} else {
		if m == "" {
			m = keepNewer
		}
		for i, p := range paths {
			paths[i] = strings.Trim(filepath.ToSlash(filepath.Clean(p)), "/")
			if paths[i] == "" || paths[i] == "." || strings.HasPrefix(paths[i], "..") {
				return fmt.Errorf("invalid --path %q", p)
			}
			if _, err := os.Lstat(filepath.Join(e.backupHome(), paths[i])); err != nil {
				return fmt.Errorf("%s is not in the backup", p)
			}
		}
	}
	return restorePaths(e, *target, paths, m, *dry, *yes)
}

func requireBackup(e env) error {
	if _, err := os.Stat(e.backupHome()); err != nil {
		return fmt.Errorf("no backup at %s — is the drive connected?", e.backupHome())
	}
	return nil
}

func restoreMenu(e env) error {
	if len(librariesIn(e.photosDir())) > 0 {
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
	return cmdRestore(e, nil)
}

func restorePaths(e env, target string, paths []string, m conflictMode, dry, yes bool) error {
	bin, err := findRsync(e)
	if err != nil {
		return err
	}
	var plan restorePlan
	if err := withSpinner("Comparing the backup with "+target+"…", func() (err error) {
		plan, err = planRestore(e, bin, target, paths)
		return err
	}); err != nil {
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
	if m == "" {
		if m, err = chooseMode(plan); err != nil {
			return err
		}
	} else if !yes {
		if err := confirmOrCancel(fmt.Sprintf("Restore %s into %s?", humanBytes(plan.bytesFor(m)), target), "", true); err != nil {
			return err
		}
	}
	if m == overwrite && plan.newerHere > 0 && !yes {
		if err := confirmOrCancel(fmt.Sprintf("Replace %d files that are newer on this Mac?", plan.newerHere), "Their newer versions will be lost.", false); err != nil {
			return err
		}
	}
	return runRestore(e, bin, target, paths, m, plan.skippedLibraries...)
}

func chooseMode(plan restorePlan) (conflictMode, error) {
	m := keepNewer
	err := huh.NewSelect[conflictMode]().Title("How should existing files be handled?").Options(
		huh.NewOption(fmt.Sprintf("Keep files that are newer on this Mac — copies %s", humanBytes(plan.bytesFor(keepNewer))), keepNewer),
		huh.NewOption(fmt.Sprintf("Only add missing files, never touch existing ones — copies %s", humanBytes(plan.bytesFor(onlyMissing))), onlyMissing),
		huh.NewOption(fmt.Sprintf("Overwrite with the backup version — copies %s", humanBytes(plan.bytesFor(overwrite))), overwrite),
	).Value(&m).Run()
	return m, err
}

func writeFileList(paths []string) (string, error) {
	return writeTemp("macup-restore-*.list", []byte(strings.Join(paths, "\x00")+"\x00"))
}

func restoreArgs(e env, list, target string, m conflictMode, skip ...string) []string {
	args := rsyncArgs("-r", "--from0", "--files-from="+list)
	for _, lib := range skip {
		args = append(args, "--exclude=/"+lib+"/")
	}
	args = append(args, m.flag()...)
	return append(args, e.backupHome()+"/", strings.TrimSuffix(target, "/")+"/")
}

func planRestore(e env, bin, target string, paths []string) (restorePlan, error) {
	list, err := writeFileList(paths)
	if err != nil {
		return restorePlan{}, err
	}
	defer os.Remove(list)
	var plan restorePlan
	err = dryRun(bin, restoreArgs(e, list, target, overwrite), func(it rsyncItem) {
		plan.add(it, e.backupHome(), target)
	})
	return plan, err
}

func (p *restorePlan) add(it rsyncItem, src, target string) {
	if root, inLibrary := libraryRoot(it.name); inLibrary {
		if slices.Contains(p.skippedLibraries, root) {
			return
		}
		if _, err := os.Stat(filepath.Join(target, root)); err == nil {
			p.skippedLibraries = append(p.skippedLibraries, root)
			return
		}
	}
	if !it.file() {
		return
	}
	if it.isNew() {
		p.newFiles++
		p.newBytes += it.size
		return
	}
	p.changed++
	p.changedBytes += it.size
	here, err1 := os.Lstat(filepath.Join(target, it.name))
	there, err2 := os.Lstat(filepath.Join(src, it.name))
	if err1 == nil && err2 == nil && here.ModTime().After(there.ModTime()) {
		p.newerHere++
		p.newerHereBytes += it.size
		if len(p.newerSamples) < 8 {
			p.newerSamples = append(p.newerSamples, it.name)
		}
	}
}

func printPlan(p restorePlan) {
	step("Restore preview")
	for _, lib := range p.skippedLibraries {
		warn("skipping %s: a Photos library already exists there, and copying another library into it would damage it", lib)
	}
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

func touches(paths []string, dir string) bool {
	return slices.ContainsFunc(paths, func(p string) bool { return p == dir || isUnder(p, dir) })
}

func runRestore(e env, bin, target string, paths []string, m conflictMode, skip ...string) error {
	list, err := writeFileList(paths)
	if err != nil {
		return err
	}
	defer os.Remove(list)
	if touches(paths, "Library") {
		if g := fullDiskAccessGuard(e, "restore"); g != nil {
			warn("%s", g.problem)
		}
	}
	l, err := acquireLock(e.lockDir())
	if err != nil {
		return err
	}
	defer l.release()
	job := rsyncJob{bin: bin, args: restoreArgs(e, list, target, m, skip...), logPath: logPath(e, "restore")}
	res, err := runJob(fmt.Sprintf("Restoring %s into %s", plural(len(paths), "item"), target), job)
	if err != nil {
		return err
	}
	if res.ok() && touches(paths, ".ssh") {
		fixSSHPermissions(filepath.Join(target, ".ssh"))
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
