package main

import (
	"bytes"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"slices"
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

func nixArgs(args ...string) []string { return slices.Concat(nixFlags, args) }

func nixVersion(nix string) string {
	out, err := exec.Command(nix, "--version").Output()
	if err != nil {
		return "unknown"
	}
	return firstLine(out)
}

func flakeHost(e env, nix string) (string, error) {
	out, err := exec.Command(nix, nixArgs("eval", "--json", e.nixDir()+"#darwinConfigurations", "--apply", "builtins.attrNames")...).Output()
	if err != nil {
		return "", fmt.Errorf("cannot read darwinConfigurations from %s: %w", e.nixDir(), err)
	}
	var names []string
	if err := json.Unmarshal(out, &names); err != nil {
		return "", err
	}
	if len(names) == 1 {
		return names[0], nil
	}
	local, _ := exec.Command("scutil", "--get", "LocalHostName").Output()
	if host := strings.TrimSpace(string(local)); slices.Contains(names, host) {
		return host, nil
	}
	return "", fmt.Errorf("flake.nix defines %s and none matches this Mac's name; keep one or rename this Mac", strings.Join(names, ", "))
}

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
	ok("%s", nixVersion(nix))

	step("Configuration")
	if err := ensureNixConfig(e); err != nil {
		return err
	}
	host, err := flakeHost(e, nix)
	if err != nil {
		return err
	}
	ok("%s#%s", e.nixDir(), host)

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
		if err := runAttached(nix, nixArgs("build", "--no-link", e.nixDir()+"#darwinConfigurations."+host+".system")...); err != nil {
			return fmt.Errorf("the configuration does not build: %w", err)
		}
		ok("configuration builds")
		return nil
	}

	step("Applying system configuration (sudo may ask for your password)")
	rebuild := append([]string{"-H", nix}, nixArgs("run", "--inputs-from", e.nixDir(), "nix-darwin#darwin-rebuild", "--", "switch", "--flake", e.nixDir()+"#"+host)...)
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
	if e.hasNixConfig() {
		return nil
	}
	if !hasFlake(e.driveNixDir()) {
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
	note("flake.lock is owned by root; giving it back to %s", e.user())
	return runAttached("sudo", "chown", fmt.Sprint(os.Getuid()), lock)
}

func syncNixConfigToDrive(e env, rsyncBin string) error {
	if !e.hasNixConfig() {
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

func nixConfigInSync(e env) bool {
	same := true
	filepath.WalkDir(e.nixDir(), func(p string, d fs.DirEntry, err error) error {
		if err != nil || d.IsDir() {
			return err
		}
		rel, _ := filepath.Rel(e.nixDir(), p)
		a, err1 := os.ReadFile(p)
		b, err2 := os.ReadFile(filepath.Join(e.driveNixDir(), rel))
		if err1 != nil || err2 != nil || !bytes.Equal(a, b) {
			same = false
			return fs.SkipAll
		}
		return nil
	})
	return same
}

func offerShellRestore(e env) error {
	if _, err := os.Stat(filepath.Join(e.home, ".oh-my-zsh")); err == nil {
		return nil
	}
	paths := existingPaths(e.backupHome(), shellPreset.paths)
	if len(paths) == 0 {
		return nil
	}
	if !interactive() {
		note("restore your shell config with: macup restore --path %s", strings.Join(paths, " --path "))
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
	return runRestore(e, bin, e.home, paths, keepNewer)
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
	editor := envOr("VISUAL", os.Getenv("EDITOR"))
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

