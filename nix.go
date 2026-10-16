package main

import (
	"bytes"
	"errors"
	"flag"
	"fmt"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"syscall"

	"github.com/charmbracelet/huh"
)

var nixFlags = []string{"--extra-experimental-features", "nix-command flakes"}

func findNix() string {
	if p, err := exec.LookPath("nix"); err == nil {
		return p
	}
	for _, p := range []string{"/nix/var/nix/profiles/default/bin/nix", "/run/current-system/sw/bin/nix"} {
		if _, err := os.Stat(p); err == nil {
			return p
		}
	}
	return ""
}

func runAttached(name string, args ...string) error {
	fmt.Println(styleDim.Render("  $ " + name + " " + strings.Join(args, " ")))
	cmd := exec.Command(name, args...)
	cmd.Stdin, cmd.Stdout, cmd.Stderr = os.Stdin, os.Stdout, os.Stderr
	return cmd.Run()
}

func nixArgs(args ...string) []string { return append(append([]string{}, nixFlags...), args...) }

func cmdApply(e env, args []string) error {
	fs := flag.NewFlagSet("apply", flag.ContinueOnError)
	update := fs.Bool("update", false, "update all pinned packages to their latest versions first")
	check := fs.Bool("check", false, "only build the configuration to check it, without switching to it")
	if err := fs.Parse(args); err != nil {
		return err
	}
	return runApply(e, *update, *check)
}

func runApply(e env, update, check bool) error {
	if runtime.GOARCH != "arm64" {
		return errors.New("this configuration targets Apple Silicon; update flake.nix before using it on Intel")
	}
	step("Apple Command Line Tools")
	if exec.Command("xcode-select", "-p").Run() != nil {
		runAttached("xcode-select", "--install")
		return errors.New("finish Apple's Command Line Tools installer, then run `macup apply` again")
	}
	ok("installed")

	step("Nix")
	nix := findNix()
	if nix == "" {
		note("installing Lix, a Nix implementation")
		if err := runAttached("/bin/sh", "-c", "curl --proto '=https' --tlsv1.2 -sSf -L https://install.lix.systems/lix | sh -s -- install"); err != nil {
			return fmt.Errorf("installing Lix failed: %w", err)
		}
		if nix = findNix(); nix == "" {
			return errors.New("Lix was installed but is not on PATH yet — open a new terminal and run `macup apply` again")
		}
	}
	if out, err := exec.Command(nix, "--version").Output(); err == nil {
		ok("%s", firstLine(out))
	}

	step("Configuration")
	if err := ensureNixConfig(e); err != nil {
		return err
	}
	ok("%s", e.nixDir())

	if update {
		if err := ensureLockOwned(e); err != nil {
			return err
		}
		step("Updating pinned packages")
		if err := runAttached(nix, nixArgs("flake", "update", "--flake", e.nixDir())...); err != nil {
			return fmt.Errorf("updating failed: %w", err)
		}
	}

	if check {
		step("Building (check only, nothing is changed)")
		if err := runAttached(nix, nixArgs("build", "--no-link", e.nixDir()+"#darwinConfigurations."+e.host+".system")...); err != nil {
			return fmt.Errorf("the configuration does not build: %w", err)
		}
		ok("configuration builds")
		return nil
	}

	step("Applying system configuration (sudo may ask for your password)")
	rebuild := append([]string{"-H", nix}, nixArgs("run", "--inputs-from", e.nixDir(), "nix-darwin#darwin-rebuild", "--", "switch", "--flake", e.nixDir()+"#"+e.host)...)
	if err := runAttached("sudo", rebuild...); err != nil {
		return fmt.Errorf("applying failed: %w", err)
	}
	if err := ensureLockOwned(e); err != nil {
		return err
	}
	ok("system configuration applied")

	if checkMounted(e.volume) == nil {
		if bin, err := findRsync(e); err == nil {
			if err := syncNixConfigToDrive(e, bin); err != nil {
				warn("could not copy the config to the drive: %v", err)
			} else {
				ok("config saved to %s", e.driveNixDir())
			}
		}
	}
	return offerShellRestore(e)
}

func ensureNixConfig(e env) error {
	if _, err := os.Stat(filepath.Join(e.nixDir(), "flake.nix")); err == nil {
		return nil
	}
	if _, err := os.Stat(filepath.Join(e.driveNixDir(), "flake.nix")); err != nil {
		return fmt.Errorf("no config at %s and none on the backup drive (%s) — connect the drive", e.nixDir(), e.driveNixDir())
	}
	note("copying the config from the backup drive")
	return copyDir(e.driveNixDir(), e.nixDir())
}

func copyDir(src, dst string) error {
	return filepath.WalkDir(src, func(p string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		rel, _ := filepath.Rel(src, p)
		target := filepath.Join(dst, rel)
		if d.IsDir() {
			return os.MkdirAll(target, 0o755)
		}
		if !d.Type().IsRegular() {
			return nil
		}
		data, err := os.ReadFile(p)
		if err != nil {
			return err
		}
		info, _ := d.Info()
		return os.WriteFile(target, data, info.Mode().Perm())
	})
}

func ensureLockOwned(e env) error {
	lock := filepath.Join(e.nixDir(), "flake.lock")
	info, err := os.Stat(lock)
	if err != nil {
		return nil
	}
	if st, ok := info.Sys().(*syscall.Stat_t); ok && int(st.Uid) == os.Getuid() {
		return nil
	}
	note("flake.lock is owned by root; giving it back to %s", e.user)
	return runAttached("sudo", "chown", fmt.Sprint(os.Getuid()), lock)
}

func syncNixConfigToDrive(e env, rsyncBin string) error {
	if _, err := os.Stat(filepath.Join(e.nixDir(), "flake.nix")); err != nil {
		return nil
	}
	if err := os.MkdirAll(e.driveNixDir(), 0o755); err != nil {
		return err
	}
	out, err := exec.Command(rsyncBin, "-a", "--delete", e.nixDir()+"/", e.driveNixDir()+"/").CombinedOutput()
	if err != nil {
		return fmt.Errorf("%v: %s", err, strings.TrimSpace(string(out)))
	}
	return nil
}

func nixConfigInSync(e env) (bool, error) {
	var differs bool
	err := filepath.WalkDir(e.nixDir(), func(p string, d fs.DirEntry, err error) error {
		if err != nil || d.IsDir() {
			return err
		}
		rel, _ := filepath.Rel(e.nixDir(), p)
		a, err1 := os.ReadFile(p)
		b, err2 := os.ReadFile(filepath.Join(e.driveNixDir(), rel))
		if err1 != nil || err2 != nil || !bytes.Equal(a, b) {
			differs = true
		}
		return nil
	})
	return !differs, err
}

func offerShellRestore(e env) error {
	if _, err := os.Stat(filepath.Join(e.home, ".oh-my-zsh")); err == nil {
		return nil
	}
	src := e.backupHome()
	var paths []string
	for _, p := range presets[0].paths {
		if _, err := os.Lstat(filepath.Join(src, p)); err == nil {
			paths = append(paths, p)
		}
	}
	if len(paths) == 0 {
		return nil
	}
	if !interactive() {
		note("restore your shell config with: macup restore --path .zshrc --path .oh-my-zsh")
		return nil
	}
	yes, err := confirm("Restore your shell config from the backup?", strings.Join(paths, ", "), true)
	if err != nil || !yes {
		return nil
	}
	bin, err := findRsync(e)
	if err != nil {
		return err
	}
	return runRestore(e, bin, src, e.home, paths, keepNewer)
}

func cmdEdit(e env, args []string) error {
	files := map[string]string{
		"packages": "home.nix",
		"apps":     "flake.nix",
		"system":   "darwin.nix",
	}
	choice := "packages"
	if len(args) > 0 {
		choice = args[0]
	} else if interactive() {
		if err := huh.NewSelect[string]().Title("What do you want to change?").Options(
			huh.NewOption("Command-line tools (home.nix)", "packages"),
			huh.NewOption("Apps installed with Homebrew (flake.nix)", "apps"),
			huh.NewOption("macOS and Nix settings (darwin.nix)", "system"),
		).Value(&choice).Run(); err != nil {
			return err
		}
	}
	name, found := files[choice]
	if !found {
		return fmt.Errorf("unknown section %q (packages, apps or system)", choice)
	}
	if err := ensureNixConfig(e); err != nil {
		return err
	}
	editor := os.Getenv("VISUAL")
	if editor == "" {
		editor = os.Getenv("EDITOR")
	}
	if editor == "" {
		editor = "vi"
		if _, err := exec.LookPath("code"); err == nil {
			editor = "code --wait"
		}
	}
	if err := runAttached("/bin/sh", "-c", editor+` "$1"`, "macup-edit", filepath.Join(e.nixDir(), name)); err != nil {
		return err
	}
	return offerApply(e)
}

func offerApply(e env) error {
	if !interactive() {
		note("run `macup apply` to make the change take effect")
		return nil
	}
	yes, err := confirm("Apply the change now?", "", true)
	if err != nil || !yes {
		note("run `macup apply` later to make the change take effect")
		return nil
	}
	return runApply(e, false, false)
}

func cmdAdd(e env, args []string) error    { return changePackages(e, args, true) }
func cmdRemove(e env, args []string) error { return changePackages(e, args, false) }

func changePackages(e env, args []string, add bool) error {
	verb, past := "remove", "Removed"
	if add {
		verb, past = "add", "Added"
	}
	fs := flag.NewFlagSet(verb, flag.ContinueOnError)
	app := fs.Bool("app", false, "a Mac app from Homebrew (cask) instead of a command-line tool")
	if err := fs.Parse(args); err != nil {
		return err
	}
	names := fs.Args()
	if len(names) == 0 {
		return fmt.Errorf("usage: macup %s [--app] <name>...", verb)
	}
	if err := ensureNixConfig(e); err != nil {
		return err
	}
	file := filepath.Join(e.nixDir(), "home.nix")
	edit := editPackages
	if *app {
		file = filepath.Join(e.nixDir(), "flake.nix")
		edit = editCasks
	}
	if add {
		for _, n := range names {
			if err := validatePackage(e, n, *app); err != nil {
				return err
			}
		}
	}
	src, err := os.ReadFile(file)
	if err != nil {
		return err
	}
	out, changed, err := edit(string(src), names, add)
	if err != nil {
		return err
	}
	if len(changed) == 0 {
		ok("nothing to change")
		return nil
	}
	if err := os.WriteFile(file, []byte(out), 0o644); err != nil {
		return err
	}
	ok("%s %s in %s", past, strings.Join(changed, ", "), filepath.Base(file))
	return offerApply(e)
}

func validatePackage(e env, name string, app bool) error {
	if app {
		brew, err := exec.LookPath("brew")
		if err != nil {
			return nil
		}
		if exec.Command(brew, "info", "--cask", name).Run() != nil {
			return fmt.Errorf("Homebrew has no app called %q (try `brew search %s`)", name, name)
		}
		return nil
	}
	nix := findNix()
	if nix == "" {
		return errors.New("Nix is not installed yet — run `macup apply` first")
	}
	out, err := exec.Command(nix, nixArgs("eval", "--raw", "--inputs-from", e.nixDir(), "nixpkgs#"+name+".name")...).Output()
	if err != nil {
		return fmt.Errorf("nixpkgs has no package called %q (search at https://search.nixos.org/packages?query=%s)", name, name)
	}
	note("found %s", strings.TrimSpace(string(out)))
	return nil
}

func cmdStatus(e env, _ []string) error {
	row := func(label, value string) { fmt.Printf("%s %s\n", styleStep.Render(fmt.Sprintf("%-14s", label)), value) }
	fmt.Println(styleTitle.Render("macup status"))
	if err := checkMounted(e.volume); err != nil {
		row("Backup drive", styleWarn.Render("not connected"))
	} else {
		v := e.volume + " · connected"
		if free, total, err := freeSpace(e.volume); err == nil && total > 0 {
			v += fmt.Sprintf(" · %s free of %s", humanBytes(int64(free)), humanBytes(int64(total)))
			if free*10 < total {
				v = styleWarn.Render(v + " · nearly full")
			}
		}
		row("Backup drive", v)
	}
	row("Last backup", lastBackup(e))
	row("Photos", lastPhotoBackup(e))
	if nix := findNix(); nix != "" {
		out, _ := exec.Command(nix, "--version").Output()
		row("Nix", firstLine(out))
	} else {
		row("Nix", styleWarn.Render("not installed — run macup apply"))
	}
	if gen, err := os.Readlink("/run/current-system"); err == nil {
		row("System", filepath.Base(gen))
	} else {
		row("System", styleWarn.Render("nix-darwin not applied yet"))
	}
	if _, err := os.Stat(filepath.Join(e.nixDir(), "flake.nix")); err != nil {
		row("Config", styleWarn.Render("missing — macup apply copies it from the drive"))
	} else {
		v := e.nixDir()
		if checkMounted(e.volume) == nil {
			if same, _ := nixConfigInSync(e); same {
				v += " · same as drive copy"
			} else {
				v += styleWarn.Render(" · differs from drive copy (saved on next apply or backup)")
			}
		}
		row("Config", v)
	}
	return nil
}

func lastRealBackupLog(e env) (string, bool) {
	logs, _ := filepath.Glob(filepath.Join(e.logDir(), "backup-*.log"))
	for i := len(logs) - 1; i >= 0; i-- {
		data, err := os.ReadFile(logs[i])
		if err != nil {
			continue
		}
		lines := strings.Split(strings.TrimSpace(string(data)), "\n")
		last := lines[len(lines)-1]
		if strings.Contains(lines[0], "--dry-run") || !strings.Contains(last, ": finished") {
			continue
		}
		return last, true
	}
	return "", false
}

func lastBackup(e env) string {
	if last, found := lastRealBackupLog(e); found {
		return last
	}
	if info, err := os.Stat(e.backupHome()); err == nil {
		return "unknown (backup folder last changed " + info.ModTime().Format("2006-01-02 15:04") + ")"
	}
	return "none"
}

func firstLine(b []byte) string {
	line, _, _ := strings.Cut(strings.TrimSpace(string(b)), "\n")
	return line
}
