package main

import (
	"errors"
	"fmt"
	"io"
	"io/fs"
	"os"
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

func terminalName() string { return envOr("TERM_PROGRAM", "your terminal app") }

func fullDiskAccessGuard(e env, what string) *guard {
	if granted, known := hasFullDiskAccess(e.home); granted || !known {
		return nil
	}
	return &guard{
		problem: fmt.Sprintf("%s does not have Full Disk Access, so macOS blocks parts of ~/Library (Contacts, Messages, Mail, Safari, call history) and the %s would skip them. "+
			"Turn it on in System Settings → Privacy & Security → Full Disk Access, then quit and reopen %s.", terminalName(), what, terminalName()),
		question: "Continue without Full Disk Access?",
		fixTitle: "Open that settings page now?",
		fix:      openSettings("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles"),
	}
}
