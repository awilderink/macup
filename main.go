package main

import (
	"errors"
	"flag"
	"fmt"
	"os"

	"github.com/charmbracelet/huh"
)

var (
	errCancelled = errors.New("cancelled")
	errSilent    = errors.New("")
)

const usage = `macup — back up, restore and set up this Mac

Usage:
  macup                      interactive menu
  macup backup [-n] [--mirror] [--exclude PATTERN]
                             copy your home folder to the backup drive
  macup restore [-n] [--path P]... [--mode update|missing|overwrite] [--target DIR] [--yes]
                             pick files and folders to bring back (no --path opens the picker)
  macup apply [--update] [--check]
                             install whatever is missing (Command Line Tools, Nix, config)
                             and apply the system configuration; --update upgrades packages
  macup add [--app] NAME...  add a command-line tool (or a Homebrew app) and apply
  macup remove [--app] NAME...
  macup edit [packages|apps|system]
                             open the Nix config in your editor
  macup status               drive, last backup, Nix and config at a glance

What the backup skips is compiled in from backup.rules in the macup repo.
`

func main() {
	e, err := loadEnv()
	if err == nil {
		if len(os.Args) < 2 {
			err = menu(e)
		} else {
			err = dispatch(e, os.Args[1], os.Args[2:])
		}
	}
	switch {
	case err == nil:
	case errors.Is(err, errCancelled), errors.Is(err, huh.ErrUserAborted):
		fmt.Println(styleDim.Render("cancelled"))
		os.Exit(130)
	case errors.Is(err, flag.ErrHelp):
	case errors.Is(err, errSilent):
		os.Exit(1)
	default:
		fmt.Fprintln(os.Stderr, styleError.Render("✗ ")+err.Error())
		os.Exit(1)
	}
}

func dispatch(e env, cmd string, args []string) error {
	switch cmd {
	case "backup":
		return cmdBackup(e, args)
	case "restore":
		return cmdRestore(e, args)
	case "apply", "install", "setup":
		return cmdApply(e, args)
	case "add":
		return cmdAdd(e, args)
	case "remove", "rm":
		return cmdRemove(e, args)
	case "edit":
		return cmdEdit(e, args)
	case "status":
		return cmdStatus(e, args)
	case "help", "-h", "--help":
		fmt.Print(usage)
		return nil
	}
	return fmt.Errorf("unknown command %q — run `macup help`", cmd)
}

func menu(e env) error {
	if !interactive() {
		fmt.Print(usage)
		return nil
	}
	for {
		fmt.Println()
		cmdStatus(e, nil)
		fmt.Println()
		choice := ""
		err := huh.NewSelect[string]().Title("What do you want to do?").Options(
			huh.NewOption("Back up this Mac", "backup"),
			huh.NewOption("Restore files and folders from the backup", "restore"),
			huh.NewOption("Install / apply the system configuration", "apply"),
			huh.NewOption("Update all packages", "update"),
			huh.NewOption("Add a command-line tool or app", "add"),
			huh.NewOption("Edit the configuration", "edit"),
			huh.NewOption("Quit", "quit"),
		).Value(&choice).Run()
		if err != nil || choice == "quit" {
			return nil
		}
		err = runMenuChoice(e, choice)
		switch {
		case err == nil, errors.Is(err, errSilent):
		case errors.Is(err, errCancelled), errors.Is(err, huh.ErrUserAborted):
			fmt.Println(styleDim.Render("cancelled"))
		default:
			fmt.Fprintln(os.Stderr, styleError.Render("✗ ")+err.Error())
		}
	}
}

func runMenuChoice(e env, choice string) error {
	switch choice {
	case "backup":
		return backupMenu(e)
	case "restore":
		return restoreMenu(e)
	case "apply":
		return runApply(e, false, false)
	case "update":
		return runApply(e, true, false)
	case "add":
		return addMenu(e)
	case "edit":
		return cmdEdit(e, nil)
	}
	return nil
}

func addMenu(e env) error {
	kind, name := "tool", ""
	form := huh.NewForm(huh.NewGroup(
		huh.NewSelect[string]().Title("What kind?").Options(
			huh.NewOption("Command-line tool from nixpkgs", "tool"),
			huh.NewOption("Mac app from Homebrew", "app"),
		).Value(&kind),
		huh.NewInput().Title("Name").Description("e.g. htop, or firefox for an app").Value(&name),
	))
	if err := form.Run(); err != nil {
		return err
	}
	if name == "" {
		return errCancelled
	}
	args := []string{name}
	if kind == "app" {
		args = []string{"--app", name}
	}
	return cmdAdd(e, args)
}
