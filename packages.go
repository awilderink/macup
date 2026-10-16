package main

import (
	"errors"
	"flag"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"slices"
	"strconv"
	"strings"
)

var (
	packagesBlock = regexp.MustCompile(`(?s)(home\.packages = with pkgs; \[\n)(.*?)(\n([ \t]*)\];)`)
	casksList     = regexp.MustCompile(`(casks = \[)([^\]]*)(\];)`)
)

func editPackages(src string, names []string, add bool) (string, []string, error) {
	m := packagesBlock.FindStringSubmatchIndex(src)
	if m == nil {
		return "", nil, errors.New("could not find `home.packages = with pkgs; [ … ];` in home.nix — edit it by hand with `macup edit`")
	}
	body := src[m[4]:m[5]]
	indent := src[m[8]:m[9]] + "  "
	current := strings.Fields(body)
	next, changed := applyChange(current, names, add)
	if len(changed) == 0 {
		return src, nil, nil
	}
	lines := make([]string, len(next))
	for i, n := range next {
		lines[i] = indent + n
	}
	return src[:m[4]] + strings.Join(lines, "\n") + src[m[5]:], changed, nil
}

func editCasks(src string, names []string, add bool) (string, []string, error) {
	m := casksList.FindStringSubmatchIndex(src)
	if m == nil {
		return "", nil, errors.New("could not find `casks = [ … ];` in flake.nix — edit it by hand with `macup edit apps`")
	}
	var current []string
	for _, f := range strings.Fields(src[m[4]:m[5]]) {
		if s, err := strconv.Unquote(f); err == nil {
			current = append(current, s)
		}
	}
	next, changed := applyChange(current, names, add)
	if len(changed) == 0 {
		return src, nil, nil
	}
	quoted := make([]string, len(next))
	for i, n := range next {
		quoted[i] = strconv.Quote(n)
	}
	return src[:m[4]] + " " + strings.Join(quoted, " ") + " " + src[m[5]:], changed, nil
}

func applyChange(current, names []string, add bool) ([]string, []string) {
	var changed []string
	next := slices.Clone(current)
	for _, n := range names {
		has := slices.Contains(next, n)
		switch {
		case add && !has:
			next = append(next, n)
			changed = append(changed, n)
		case !add && has:
			next = slices.DeleteFunc(next, func(s string) bool { return s == n })
			changed = append(changed, n)
		}
	}
	slices.SortFunc(next, func(a, b string) int { return strings.Compare(strings.ToLower(a), strings.ToLower(b)) })
	return next, changed
}

func cmdAdd(e env, args []string) error    { return parsePackageCommand(e, "add", args) }
func cmdRemove(e env, args []string) error { return parsePackageCommand(e, "remove", args) }

func parsePackageCommand(e env, verb string, args []string) error {
	fs := flag.NewFlagSet(verb, flag.ContinueOnError)
	app := fs.Bool("app", false, "a Mac app from Homebrew (cask) instead of a command-line tool")
	if err := fs.Parse(args); err != nil {
		return err
	}
	if fs.NArg() == 0 {
		return fmt.Errorf("usage: macup %s [--app] <name>...", verb)
	}
	return changePackages(e, fs.Args(), *app, verb == "add")
}

func changePackages(e env, names []string, app, add bool) error {
	if err := ensureNixConfig(e); err != nil {
		return err
	}
	file := filepath.Join(e.nixDir(), "home.nix")
	edit := editPackages
	if app {
		file = filepath.Join(e.nixDir(), "flake.nix")
		edit = editCasks
	}
	if add {
		for _, n := range names {
			if err := validatePackage(e, n, app); err != nil {
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
	past := "Removed"
	if add {
		past = "Added"
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
