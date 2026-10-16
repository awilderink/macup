package main

import (
	"errors"
	"fmt"
	"io"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
)

var protectedDirs = []string{
	"Library/Application Support/AddressBook",
	"Library/Application Support/CallHistoryDB",
	"Library/Application Support/Knowledge",
	"Library/Mail",
	"Library/Messages",
	"Library/Safari",
}

const fullDiskAccessPane = "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles"

func hasFullDiskAccess(home string) (granted, known bool) {
	for _, rel := range protectedDirs {
		dir := filepath.Join(home, rel)
		if _, err := os.Lstat(dir); err != nil {
			continue
		}
		f, err := os.Open(dir)
		if err == nil {
			_, err = f.ReadDir(1)
			f.Close()
		}
		if errors.Is(err, fs.ErrPermission) {
			return false, true
		}
		if err == nil || errors.Is(err, io.EOF) {
			return true, true
		}
	}
	return false, false
}

func terminalName() string {
	if t := os.Getenv("TERM_PROGRAM"); t != "" {
		return t
	}
	return "your terminal app"
}

func checkFullDiskAccess(e env, what string, yes bool) error {
	if granted, known := hasFullDiskAccess(e.home); granted || !known {
		return nil
	}
	msg := fmt.Sprintf("%s does not have Full Disk Access, so macOS blocks parts of ~/Library (Contacts, Messages, Mail, Safari, call history) and %s would silently skip them. "+
		"Turn it on in System Settings → Privacy & Security → Full Disk Access, then quit and reopen %s.", terminalName(), what, terminalName())
	if yes {
		warn("%s", msg)
		return nil
	}
	if !interactive() {
		return errors.New(msg + " Add --yes to continue without it.")
	}
	warn("%s", msg)
	open, err := confirm("Open that settings page now?", "", true)
	if err != nil {
		return err
	}
	if open {
		exec.Command("open", fullDiskAccessPane).Run()
		return errCancelled
	}
	cont, err := confirm("Continue without Full Disk Access?", "Protected folders will be missing from the "+what+".", false)
	if err != nil || !cont {
		return errCancelled
	}
	return nil
}
