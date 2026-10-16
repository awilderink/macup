package main

import (
	"os"
	"path/filepath"
	"testing"
)

func TestFullDiskAccessDetection(t *testing.T) {
	home := t.TempDir()
	if _, known := hasFullDiskAccess(home); known {
		t.Fatal("with no protected folders present the answer should be unknown")
	}
	dir := filepath.Join(home, "Library/Application Support/AddressBook")
	os.MkdirAll(dir, 0o755)
	if granted, known := hasFullDiskAccess(home); !granted || !known {
		t.Fatalf("readable protected folder: granted=%v known=%v", granted, known)
	}
	// macOS reports a TCC denial as EPERM; an unreadable folder gives the same
	// permission error, which is what the check relies on.
	os.Chmod(dir, 0o000)
	defer os.Chmod(dir, 0o755)
	if granted, known := hasFullDiskAccess(home); granted || !known {
		t.Fatalf("unreadable protected folder: granted=%v known=%v", granted, known)
	}
}
